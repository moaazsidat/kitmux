//! What kit needs to know about processes, read from /proc.
//!
//! Everything here answers "unknown" (None or empty) rather than failing: a
//! process can exit between two reads, and one unreadable process must never
//! stop a whole snapshot.

use std::fs;
use std::path::PathBuf;

/// The process's argv.
pub fn cmdline(pid: u32) -> Option<Vec<String>> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    (!args.is_empty()).then_some(args)
}

/// Fields of /proc/<pid>/stat after the command name, which may itself
/// contain spaces and parentheses.
fn stat_fields(pid: u32) -> Option<Vec<String>> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = &stat[stat.rfind(')')? + 1..];
    Some(after.split_whitespace().map(str::to_string).collect())
}

/// The parent process id.
pub fn ppid(pid: u32) -> Option<u32> {
    // After the name: state (0), ppid (1).
    stat_fields(pid)?.get(1)?.parse().ok()
}

/// When the process started, in clock ticks since boot. Together with the pid
/// and boot id it names a process uniquely, even after its pid is reused.
pub fn start_ticks(pid: u32) -> Option<u64> {
    // Field 22 of stat is starttime; it is index 19 after the name.
    stat_fields(pid)?.get(19)?.parse().ok()
}

/// This boot's id, so that process identities from before a reboot never
/// match ones after it.
pub fn boot_id() -> Option<String> {
    Some(fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?.trim().to_string())
}

/// The files the process has open.
pub fn open_files(pid: u32) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    entries.flatten().filter_map(|e| fs::read_link(e.path()).ok()).collect()
}

/// One variable from the process's environment.
pub fn environ_var(pid: u32, name: &str) -> Option<String> {
    let raw = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let prefix = format!("{name}=");
    raw.split(|b| *b == 0)
        .map(String::from_utf8_lossy)
        .find_map(|kv| kv.strip_prefix(&prefix).map(str::to_string))
}

/// Every process id currently visible.
pub fn pids() -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_own_process() {
        let me = std::process::id();
        assert!(cmdline(me).is_some());
        assert_eq!(ppid(me), Some(std::os::unix::process::parent_id()));
        assert!(start_ticks(me).is_some());
        assert!(pids().contains(&me));
    }
}
