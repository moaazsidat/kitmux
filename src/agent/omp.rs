//! omp.
//!
//! omp keeps the session it is writing open, at
//! `<agent dir>/sessions/<project>/<timestamp>_<session id>.jsonl`, so the
//! session id is read off the process's open files. `omp --resume <id>`
//! continues it; `omp --restore <id>` also finishes a turn that was cut off.

use std::path::Path;

use super::args::{self, Flags};
use super::{Adapter, ResumeOptions};
use crate::procfs;

pub struct Omp;

/// From `omp --help`.
const FLAGS: Flags = Flags {
    value: &[
        "--model",
        "--smol",
        "--slow",
        "--plan",
        "--goal",
        "--prewalk-into",
        "--plan-yolo-into",
        "--provider",
        "--api-key",
        "--system-prompt",
        "--system-prompt-template",
        "--append-system-prompt",
        "--profile",
        "--alias",
        "--cwd",
        "--mode",
        "--config",
        "--add-dir",
        "-r",
        "--resume",
        "--restore",
        "--session-dir",
        "--models",
        "--tools",
        "--thinking",
        "--service-tier",
        "--hook",
        "-e",
        "--extension",
        "--skills",
        "--export",
        "--max-time",
        "--approval-mode",
    ],
    variadic: &[],
    optional: &[],
    drop: &[
        "-r",
        "--resume",
        "--restore",
        "-c",
        "--continue",
        "--goal",
        "--export",
        "--no-session",
        "--from-claude",
        "--from-codex",
    ],
};

impl Adapter for Omp {
    fn kind(&self) -> &'static str {
        "omp"
    }

    fn program_end(&self, cmdline: &[String]) -> Option<usize> {
        super::find_program(cmdline, "omp", &[])
    }

    fn session_id(&self, pid: u32) -> Option<String> {
        let mut sessions: Vec<_> = procfs::open_files(pid)
            .into_iter()
            .filter_map(|p| Some((session_id_from(&p)?, mtime(&p))))
            .collect();
        // A process with more than one session open is writing the newest.
        sessions.sort_by_key(|(_, t)| *t);
        sessions.pop().map(|(id, _)| id)
    }

    fn resume(&self, program: &[String], args: &[String], session_id: &str, opts: ResumeOptions) -> Vec<String> {
        let mut cmd = program.to_vec();
        cmd.extend(args::keep(args, &FLAGS));
        let flag = if opts.continue_turn { "--restore" } else { "--resume" };
        cmd.extend([flag.to_string(), session_id.to_string()]);
        cmd
    }
}

fn mtime(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

/// The session id in a session file's path, if it is one: a `.jsonl` directly
/// inside a project directory of a `sessions` directory. Subagent transcripts
/// live deeper and are not sessions of their own.
fn session_id_from(path: &Path) -> Option<String> {
    if path.extension()? != "jsonl" {
        return None;
    }
    if path.parent()?.parent()?.file_name()? != "sessions" {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let id = stem.rsplit_once('_').map(|(_, id)| id).unwrap_or(stem);
    (!id.is_empty()).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn reads_session_ids_from_paths() {
        let p = Path::new("/home/user/.omp/agent/sessions/-work-app/2026-01-02T03-04-05-678Z_01a1-beef.jsonl");
        assert_eq!(session_id_from(p).as_deref(), Some("01a1-beef"));

        let sub = Path::new("/home/user/.omp/agent/sessions/-work-app/01a1-beef/sub_1.jsonl");
        assert_eq!(session_id_from(sub), None);
        assert_eq!(session_id_from(Path::new("/tmp/sessions/x/a_b.log")), None);
    }

    #[test]
    fn resumes_or_restores() {
        let cmd = v(&["omp", "--model", "fast", "--resume", "old", "write tests"]);
        let resume = Omp.resume(&cmd[..1], &cmd[1..], "new", ResumeOptions::default());
        assert_eq!(resume, v(&["omp", "--model", "fast", "--resume", "new"]));

        let restore = Omp.resume(&cmd[..1], &cmd[1..], "new", ResumeOptions { continue_turn: true });
        assert_eq!(restore, v(&["omp", "--model", "fast", "--restore", "new"]));
    }
}
