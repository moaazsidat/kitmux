//! Finding the kitty instances running on this machine.
//!
//! kitty appends `-<pid>` to a `listen_on` socket path unless the path already
//! contains `{kitty_pid}`, so the configured path is a pattern rather than an
//! address. kitmux is often run from outside any kitty window (a systemd timer),
//! where `KITTY_LISTEN_ON` is not set, so it reads `kitty.conf` and scans the
//! usual socket directories as well.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

use super::rc::{Addr, Client};

/// Every kitty that answers remote control. An explicit address is used alone.
pub fn find(explicit: Option<&str>) -> anyhow::Result<Vec<Client>> {
    if let Some(s) = explicit {
        return Ok(vec![Client::new(Addr::parse(s)?)]);
    }
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for addr in candidates() {
        if !seen.insert(addr.clone()) {
            continue;
        }
        let client = Client::new(addr);
        if client.call("ls", serde_json::Value::Null).is_ok() {
            found.push(client);
        }
    }
    Ok(found)
}

/// Addresses worth trying, most specific first.
fn candidates() -> Vec<Addr> {
    let mut out = Vec::new();
    if let Ok(v) = std::env::var("KITTY_LISTEN_ON")
        && let Ok(a) = Addr::parse(&v)
    {
        out.push(a);
    }
    if let Some(listen) = conf_option("listen_on").filter(|v| v != "none") {
        out.extend(expand_listen_on(&listen));
    }
    let mut dirs = vec![PathBuf::from("/tmp")];
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR") {
        dirs.push(PathBuf::from(rt));
    }
    for dir in dirs {
        out.extend(sockets_matching(&dir, |name| name.starts_with("kitty")));
    }
    out
}

fn config_file() -> Option<PathBuf> {
    let dir = if let Some(d) = std::env::var_os("KITTY_CONFIG_DIRECTORY") {
        PathBuf::from(d)
    } else if let Some(d) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(d).join("kitty")
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".config/kitty")
    };
    Some(dir.join("kitty.conf"))
}

/// An option's value in the user's kitty.conf.
pub fn conf_option(name: &str) -> Option<String> {
    parse_option(&fs::read_to_string(config_file()?).ok()?, name)
}

/// The effective `listen_on` value in a kitty.conf.
#[cfg(test)]
pub fn parse_listen_on(conf: &str) -> Option<String> {
    parse_option(conf, "listen_on").filter(|v| v != "none")
}

/// An option's effective value in a kitty.conf: the last one wins, as in
/// kitty. `include`d files are not followed.
fn parse_option(conf: &str, name: &str) -> Option<String> {
    conf.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix(name)?;
            if !rest.starts_with(char::is_whitespace) {
                return None;
            }
            Some(rest.trim().to_string())
        })
        .rfind(|v| !v.is_empty())
}

/// The addresses a `listen_on` value can produce once kitty has added its pid.
fn expand_listen_on(value: &str) -> Vec<Addr> {
    let value = expand_vars(value);
    let Ok(addr) = Addr::parse(&value) else {
        return Vec::new();
    };
    match addr {
        Addr::Path(path) => {
            let s = path.to_string_lossy().to_string();
            let (dir, prefix) = match s.find("{kitty_pid}") {
                Some(i) => {
                    let p = Path::new(&s[..i]);
                    let dir = p.parent().unwrap_or(Path::new("/")).to_path_buf();
                    let base = p
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default();
                    (dir, base)
                }
                None => {
                    let dir = path.parent().unwrap_or(Path::new("/")).to_path_buf();
                    let base = path
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default();
                    (dir, base)
                }
            };
            sockets_matching(&dir, |name| name.starts_with(&prefix))
        }
        Addr::Abstract(name) => abstract_sockets(&name),
    }
}

/// Expands `~`, `$VAR`, and `${VAR}`, as kitty does for `listen_on`.
pub fn expand_vars(s: &str) -> String {
    let mut s = s.to_string();
    if let Some(rest) = s.strip_prefix("unix:~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        s = format!("unix:{}/{rest}", home.to_string_lossy());
    }
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let braced = chars.peek() == Some(&'{');
        if braced {
            chars.next();
        }
        let mut name = String::new();
        while let Some(&n) = chars.peek() {
            if n.is_ascii_alphanumeric() || n == '_' {
                name.push(n);
                chars.next();
            } else {
                break;
            }
        }
        if braced && chars.peek() == Some(&'}') {
            chars.next();
        }
        if name.is_empty() {
            out.push('$');
        } else {
            out.push_str(&std::env::var(&name).unwrap_or_default());
        }
    }
    out
}

fn sockets_matching(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<Addr> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Addr> = entries
        .flatten()
        .filter(|e| keep(&e.file_name().to_string_lossy()))
        .filter(|e| e.file_type().map(|t| t.is_socket()).unwrap_or(false))
        .map(|e| Addr::Path(e.path()))
        .collect();
    out.sort_by_key(|a| a.to_string());
    out
}

/// Abstract sockets whose name starts with `name`, from /proc/net/unix.
fn abstract_sockets(name: &str) -> Vec<Addr> {
    let Ok(table) = fs::read_to_string("/proc/net/unix") else {
        return vec![Addr::Abstract(name.to_string())];
    };
    let mut out: Vec<Addr> = table
        .lines()
        .filter_map(|l| l.split_whitespace().nth(7))
        .filter_map(|p| p.strip_prefix('@'))
        .filter(|p| p.starts_with(name))
        .map(|p| Addr::Abstract(p.to_string()))
        .collect();
    out.sort_by_key(|a| a.to_string());
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_listen_on_wins() {
        let conf = "# listen_on unix:/commented\nlisten_on unix:/tmp/a\nlisten_on_foo x\nlisten_on   unix:/tmp/b  \n";
        assert_eq!(parse_listen_on(conf).as_deref(), Some("unix:/tmp/b"));
        assert_eq!(parse_listen_on("listen_on none\n"), None);
        assert_eq!(parse_listen_on(""), None);
    }

    #[test]
    fn expands_variables() {
        // SAFETY: tests in this module do not read KITMUX_TEST_VAR concurrently.
        unsafe { std::env::set_var("KITMUX_TEST_VAR", "/run/x") };
        assert_eq!(expand_vars("unix:${KITMUX_TEST_VAR}/kitty"), "unix:/run/x/kitty");
        assert_eq!(expand_vars("unix:$KITMUX_TEST_VAR/kitty"), "unix:/run/x/kitty");
        assert_eq!(expand_vars("unix:/tmp/kitty"), "unix:/tmp/kitty");
    }

    #[test]
    fn finds_pid_suffixed_sockets() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("kitty-1234");
        let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        fs::write(dir.path().join("kitty-not-a-socket"), "").unwrap();

        let value = format!("unix:{}/kitty", dir.path().display());
        assert_eq!(expand_listen_on(&value), vec![Addr::Path(sock.clone())]);

        let value = format!("unix:{}/kitty-{{kitty_pid}}", dir.path().display());
        assert_eq!(expand_listen_on(&value), vec![Addr::Path(sock)]);
    }
}
