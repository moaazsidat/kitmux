//! One module per subcommand.

pub mod ls;
pub mod resume;
pub mod save;
pub mod tab;
pub mod timer;

use std::fmt::Write;

/// Shows a path with the home directory as `~`.
pub fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && (path == home || path.starts_with(&format!("{home}/"))) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_string(),
    }
}

/// Quotes argv for display, the way a shell would need it.
pub fn shell_join(args: &[String]) -> String {
    let mut out = String::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let safe = !a.is_empty()
            && a.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%^".contains(c));
        if safe {
            out.push_str(a);
        } else {
            let _ = write!(out, "'{}'", a.replace('\'', r"'\''"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_for_the_shell() {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(shell_join(&v(&["claude", "--resume", "abc-1"])), "claude --resume abc-1");
        assert_eq!(shell_join(&v(&["echo", "it's here", ""])), r"echo 'it'\''s here' ''");
    }
}
