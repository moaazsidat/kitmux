//! Coding agents kit knows how to resume.
//!
//! An adapter recognises its agent from a process's argv, finds the session
//! that process is writing, and builds the command that resumes it. Adding an
//! agent means adding one adapter to [`ADAPTERS`].

mod args;
mod claude;
mod omp;

use serde::{Deserialize, Serialize};

use crate::procfs;

/// An agent session found running in a window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Which adapter found it, e.g. `claude` or `omp`.
    pub kind: String,
    /// The agent's own session id, as its resume flag takes it.
    pub session_id: String,
    pub pid: u32,
    /// The argv the agent was started with.
    pub cmdline: Vec<String>,
    /// Environment the resumed agent needs to find its session again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    /// The command that resumes this session, as recorded at save time. Used
    /// for an agent kind this build of kit does not know.
    pub resume: Vec<String>,
}

/// How to resume a session.
#[derive(Debug, Clone, Copy, Default)]
pub struct ResumeOptions {
    /// Also continue a turn the agent was in the middle of, where the agent
    /// supports it (omp's `--restore`).
    pub continue_turn: bool,
}

pub trait Adapter: Sync {
    fn kind(&self) -> &'static str;

    /// Where the agent's own program is in `cmdline`, so a wrapper such as
    /// `node /path/to/cli.js` is kept as a whole. None if this is not the agent.
    fn program_end(&self, cmdline: &[String]) -> Option<usize>;

    /// The session the process is writing, if it can be told.
    fn session_id(&self, pid: u32) -> Option<String>;

    /// Environment the resumed process needs, as `KEY=value`.
    fn env(&self, _pid: u32) -> Vec<String> {
        Vec::new()
    }

    /// `program` plus the flags worth keeping from `args`, plus whatever
    /// resumes `session_id`.
    fn resume(&self, program: &[String], args: &[String], session_id: &str, opts: ResumeOptions) -> Vec<String>;
}

pub static ADAPTERS: &[&dyn Adapter] = &[&claude::Claude, &omp::Omp];

pub fn adapter(kind: &str) -> Option<&'static dyn Adapter> {
    ADAPTERS.iter().copied().find(|a| a.kind() == kind)
}

/// The agent session a process is running, if it is a known agent and its
/// session can be identified.
pub fn detect(pid: u32, cmdline: &[String]) -> Option<Session> {
    ADAPTERS.iter().find_map(|a| {
        let end = a.program_end(cmdline)?;
        let session_id = a.session_id(pid)?;
        let resume = a.resume(&cmdline[..end], &cmdline[end..], &session_id, ResumeOptions::default());
        Some(Session {
            kind: a.kind().to_string(),
            session_id,
            pid,
            cmdline: cmdline.to_vec(),
            env: a.env(pid),
            resume,
        })
    })
}

impl Session {
    /// The command that resumes this session now.
    pub fn resume_command(&self, opts: ResumeOptions) -> Vec<String> {
        match adapter(&self.kind) {
            Some(a) => match a.program_end(&self.cmdline) {
                Some(end) => a.resume(&self.cmdline[..end], &self.cmdline[end..], &self.session_id, opts),
                None => self.resume.clone(),
            },
            None => self.resume.clone(),
        }
    }
}

/// Every agent session running on the machine right now, by `(kind, id)`.
pub fn running() -> Vec<Session> {
    procfs::pids()
        .into_iter()
        .filter_map(|pid| detect(pid, &procfs::cmdline(pid)?))
        .collect()
}

/// The last path component, without a script extension.
fn stem(arg: &str) -> &str {
    let base = arg.rsplit('/').next().unwrap_or(arg);
    for ext in [".js", ".mjs", ".cjs", ".ts"] {
        if let Some(s) = base.strip_suffix(ext) {
            return s;
        }
    }
    base
}

/// Finds `name` as the program in `cmdline`: either argv[0] itself, or the
/// script a JavaScript runtime was given. `markers` are path fragments that
/// identify the agent's script when its file name is generic (`cli.js`).
fn find_program(cmdline: &[String], name: &str, markers: &[&str]) -> Option<usize> {
    let first = cmdline.first()?;
    if stem(first) == name {
        return Some(1);
    }
    if matches!(stem(first), "node" | "bun" | "deno") {
        // Skip the runtime's own flags to reach the script.
        let script = cmdline.iter().skip(1).position(|a| !a.starts_with('-'))? + 1;
        let s = &cmdline[script];
        if stem(s) == name || markers.iter().any(|m| s.contains(m)) {
            return Some(script + 1);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn finds_programs() {
        assert_eq!(find_program(&v(&["claude", "--model", "x"]), "claude", &[]), Some(1));
        assert_eq!(find_program(&v(&["/opt/bin/claude"]), "claude", &[]), Some(1));
        assert_eq!(
            find_program(
                &v(&["node", "--no-warnings", "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js", "-c"]),
                "claude",
                &["@anthropic-ai/claude-code"]
            ),
            Some(3)
        );
        assert_eq!(find_program(&v(&["node", "server.js"]), "claude", &[]), None);
        assert_eq!(find_program(&v(&["zsh"]), "claude", &[]), None);
        assert_eq!(find_program(&v(&["claudette"]), "claude", &[]), None);
    }
}
