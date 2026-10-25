//! Claude Code.
//!
//! Each running Claude Code writes `<config dir>/sessions/<pid>.json`, whose
//! `sessionId` is the conversation the process is in now. That is the reliable
//! answer even when the process was started as a bare `claude --resume` and
//! the session was picked interactively. `claude --resume <id>` continues it.

use std::path::PathBuf;

use super::args::{self, Flags};
use super::{Adapter, ResumeOptions};
use crate::procfs;

pub struct Claude;

/// From `claude --help`. Flags this list does not know are kept without a
/// value, so an unknown flag's value is dropped as if it were a prompt.
const FLAGS: Flags = Flags {
    value: &[
        "--agent",
        "--agents",
        "--append-system-prompt",
        "--autocompact",
        "--debug-file",
        "--effort",
        "--environment",
        "--fallback-model",
        "--input-format",
        "--json-schema",
        "--max-budget-usd",
        "--model",
        "-n",
        "--name",
        "--output-format",
        "--permission-mode",
        "--permission-prompts",
        "--plugin-url",
        "--remote-control-session-name-prefix",
        "--session-id",
        "--setting-sources",
        "--settings",
        "--system-prompt",
        "--system-prompt-snapshot",
    ],
    variadic: &[
        "--add-dir",
        "--allowedTools",
        "--allowed-tools",
        "--betas",
        "--disallowedTools",
        "--disallowed-tools",
        "--file",
        "--mcp-config",
        "--plugin-dir",
        "--tools",
    ],
    optional: &["-r", "--resume", "--debug", "--from-pr"],
    drop: &[
        "-r",
        "--resume",
        "-c",
        "--continue",
        "--session-id",
        "--fork-session",
        "--from-pr",
        "-p",
        "--print",
    ],
};

impl Claude {
    fn config_dir(pid: u32) -> Option<PathBuf> {
        if let Some(dir) = procfs::environ_var(pid, "CLAUDE_CONFIG_DIR") {
            return Some(PathBuf::from(dir));
        }
        let home = procfs::environ_var(pid, "HOME").or_else(|| std::env::var("HOME").ok())?;
        Some(PathBuf::from(home).join(".claude"))
    }
}

impl Adapter for Claude {
    fn kind(&self) -> &'static str {
        "claude"
    }

    fn program_end(&self, cmdline: &[String]) -> Option<usize> {
        super::find_program(cmdline, "claude", &["@anthropic-ai/claude-code"])
    }

    fn session_id(&self, pid: u32) -> Option<String> {
        let path = Self::config_dir(pid)?.join("sessions").join(format!("{pid}.json"));
        session_id_from(&std::fs::read(path).ok()?)
    }

    fn env(&self, pid: u32) -> Vec<String> {
        procfs::environ_var(pid, "CLAUDE_CONFIG_DIR")
            .map(|d| vec![format!("CLAUDE_CONFIG_DIR={d}")])
            .unwrap_or_default()
    }

    fn resume(&self, program: &[String], args: &[String], session_id: &str, _opts: ResumeOptions) -> Vec<String> {
        let mut cmd = program.to_vec();
        cmd.extend(args::keep(args, &FLAGS));
        cmd.extend(["--resume".to_string(), session_id.to_string()]);
        cmd
    }
}

fn session_id_from(pid_file: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(pid_file).ok()?;
    let id = v.get("sessionId")?.as_str()?;
    (!id.is_empty()).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn reads_the_pid_file() {
        let file = br#"{"pid":42,"sessionId":"0b6f1c2e-0000-4000-8000-000000000001","cwd":"/work"}"#;
        assert_eq!(session_id_from(file).as_deref(), Some("0b6f1c2e-0000-4000-8000-000000000001"));
        assert_eq!(session_id_from(br#"{"pid":42}"#), None);
        assert_eq!(session_id_from(b"not json"), None);
    }

    #[test]
    fn resumes_with_the_original_flags() {
        let cmd = v(&["claude", "--dangerously-skip-permissions", "--resume"]);
        let out = Claude.resume(&cmd[..1], &cmd[1..], "abc", ResumeOptions::default());
        assert_eq!(out, v(&["claude", "--dangerously-skip-permissions", "--resume", "abc"]));

        let cmd = v(&["claude", "--model", "opus", "-c", "explain this repo"]);
        let out = Claude.resume(&cmd[..1], &cmd[1..], "abc", ResumeOptions::default());
        assert_eq!(out, v(&["claude", "--model", "opus", "--resume", "abc"]));
    }
}
