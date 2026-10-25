//! `kit timer`: save on a schedule with a systemd user timer.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

const NAME: &str = "kit-save";

pub fn install(every: &str, keep: usize) -> Result<()> {
    validate_span(every)?;
    let exe = std::env::current_exe().context("finding kit's own path")?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let dir = unit_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    fs::write(dir.join(format!("{NAME}.service")), service_unit(&exe, keep))?;
    fs::write(dir.join(format!("{NAME}.timer")), timer_unit(every))?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", &format!("{NAME}.timer")])?;
    println!("saving every {every} with {} (keeping {keep})", exe.display());
    println!("units are in {}; `kit timer status` shows the next run", dir.display());
    Ok(())
}

pub fn remove() -> Result<()> {
    let dir = unit_dir()?;
    let _ = systemctl(&["disable", "--now", &format!("{NAME}.timer")]);
    for ext in ["timer", "service"] {
        let p = dir.join(format!("{NAME}.{ext}"));
        if p.exists() {
            fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
        }
    }
    systemctl(&["daemon-reload"])?;
    println!("timer removed; saved snapshots are kept");
    Ok(())
}

pub fn status() -> Result<()> {
    let out = Command::new("systemctl")
        .args(["--user", "list-timers", "--all", &format!("{NAME}.timer")])
        .output()
        .context("running systemctl")?;
    print!("{}", String::from_utf8_lossy(&out.stdout));
    let last = Command::new("systemctl")
        .args(["--user", "status", "--no-pager", "--lines=5", &format!("{NAME}.service")])
        .output()?;
    print!("{}", String::from_utf8_lossy(&last.stdout));
    Ok(())
}

fn systemctl(args: &[&str]) -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("kit timer uses systemd, which this system does not have");
    }
    let status = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .context("running systemctl")?;
    if !status.success() {
        bail!("systemctl --user {} failed", args.join(" "));
    }
    Ok(())
}

fn unit_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d).join("systemd/user"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".config/systemd/user"))
}

/// A systemd time span, such as `1h`, `30min`, or `2h 30min`.
fn validate_span(span: &str) -> Result<()> {
    let ok = !span.trim().is_empty()
        && span.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ')
        && span.chars().next().is_some_and(|c| c.is_ascii_digit());
    if !ok {
        bail!("{span:?} is not a time span; try 1h, 30min, or 2h 30min");
    }
    Ok(())
}

fn service_unit(exe: &Path, keep: usize) -> String {
    // systemd expands % specifiers, even inside quotes.
    let exe = exe.display().to_string().replace('%', "%%").replace('"', "\\\"");
    format!(
        "[Unit]\n\
         Description=Save kitty tabs and agent sessions (kit)\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart=\"{exe}\" save --timer --keep {keep}\n"
    )
}

fn timer_unit(every: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Save kitty tabs and agent sessions every {every} (kit)\n\
         \n\
         [Timer]\n\
         OnStartupSec={every}\n\
         OnUnitActiveSec={every}\n\
         AccuracySec=1min\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_units() {
        let s = service_unit(Path::new("/opt/kit 100%/kit"), 24);
        assert!(s.contains("ExecStart=\"/opt/kit 100%%/kit\" save --timer --keep 24\n"), "{s}");
        let t = timer_unit("1h");
        assert!(t.contains("OnUnitActiveSec=1h\n"));
        assert!(t.contains("WantedBy=timers.target"));
    }

    #[test]
    fn checks_spans() {
        for ok in ["1h", "30min", "2h 30min", "90s"] {
            assert!(validate_span(ok).is_ok(), "{ok}");
        }
        for bad in ["", "h", "1h; rm", "1h\nExecStart=x"] {
            assert!(validate_span(bad).is_err(), "{bad:?}");
        }
    }
}
