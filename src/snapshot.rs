//! A snapshot: everything needed to rebuild kitty's tabs and resume the agents
//! in them, plus kitty's own `ls` output verbatim, so nothing kitty reported
//! is lost even if kitmux does not use it yet.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::agent;
use crate::kitty::{self, Client};
use crate::procfs;

/// Bumped when a change would stop an older kitmux from reading a snapshot.
pub const FORMAT: u32 = 1;

/// The tab name `kitmux tab set --name` records, as a kitty user variable on the
/// tab's windows. A tab's title is otherwise whatever its program last set, so
/// only a name given this way is restored.
pub const TAB_NAME_VAR: &str = "kitmux_tab_name";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub format: u32,
    #[serde(alias = "kit_version")]
    pub kitmux_version: String,
    pub taken_at: DateTime<Utc>,
    #[serde(default)]
    pub hostname: Option<String>,
    /// What took it: `manual` or `timer`.
    pub reason: String,
    pub instances: Vec<Instance>,
}

/// One running kitty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub socket: String,
    /// The kitty process, when it could be identified.
    #[serde(default)]
    pub process: Option<ProcessIdentity>,
    pub os_windows: Vec<OsWindow>,
    /// kitty's `ls` output, unmodified.
    pub raw: Value,
}

/// A process, named so that a reused pid or a reboot never matches it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_ticks: u64,
    pub boot_id: String,
}

impl ProcessIdentity {
    pub fn of(pid: u32) -> Option<ProcessIdentity> {
        Some(ProcessIdentity {
            pid,
            start_ticks: procfs::start_ticks(pid)?,
            boot_id: procfs::boot_id()?,
        })
    }

    /// Whether this exact process is still running.
    pub fn is_running(&self) -> bool {
        ProcessIdentity::of(self.pid).as_ref() == Some(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsWindow {
    pub id: u64,
    pub is_active: bool,
    #[serde(default)]
    pub wm_class: String,
    pub tabs: Vec<Tab>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    pub id: u64,
    /// The title kitty showed. Informational: see [`TAB_NAME_VAR`].
    pub title: String,
    /// A name given with `kitmux tab set --name`, restored as the tab's title.
    #[serde(default)]
    pub name: Option<String>,
    pub is_active: bool,
    pub layout: String,
    /// kitty's description of the layout, including a splits layout's tree.
    #[serde(default)]
    pub layout_state: Value,
    pub windows: Vec<Window>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub id: u64,
    pub title: String,
    pub cwd: String,
    /// What kitty started in the window, usually a shell.
    pub cmdline: Vec<String>,
    pub is_focused: bool,
    /// What was running in it, outermost first.
    #[serde(default)]
    pub foreground: Vec<Process>,
    #[serde(default)]
    pub user_vars: Map<String, Value>,
    /// The agent session running in the window, if any.
    #[serde(default)]
    pub agent: Option<agent::Session>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub cmdline: Vec<String>,
    pub cwd: String,
}

impl Snapshot {
    /// Takes a snapshot of every kitty in `clients`. A kitty that fails to
    /// answer is skipped with a warning rather than failing the snapshot.
    pub fn capture(clients: &[Client], reason: &str) -> anyhow::Result<Snapshot> {
        let mut instances: Vec<Instance> = Vec::new();
        for c in clients {
            match Instance::capture(c) {
                Ok(i) => {
                    // One kitty can be reachable on more than one socket.
                    let dup = i.process.is_some() && instances.iter().any(|o| o.process == i.process);
                    if !dup {
                        instances.push(i);
                    }
                }
                Err(e) => eprintln!("kitmux: skipping kitty at {}: {e:#}", c.addr),
            }
        }
        anyhow::ensure!(!instances.is_empty(), "no kitty answered remote control");
        Ok(Snapshot {
            format: FORMAT,
            kitmux_version: env!("CARGO_PKG_VERSION").to_string(),
            taken_at: Utc::now(),
            hostname: std::fs::read_to_string("/proc/sys/kernel/hostname")
                .ok()
                .map(|h| h.trim().to_string()),
            reason: reason.to_string(),
            instances,
        })
    }

    pub fn tabs(&self) -> impl Iterator<Item = &Tab> {
        self.instances.iter().flat_map(|i| &i.os_windows).flat_map(|o| &o.tabs)
    }

    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        self.tabs().flat_map(|t| &t.windows)
    }

    pub fn agents(&self) -> impl Iterator<Item = &agent::Session> {
        self.windows().filter_map(|w| w.agent.as_ref())
    }

    /// Whether this snapshot was taken of a kitty that is still running.
    pub fn of_running_kitty(&self) -> bool {
        self.instances
            .iter()
            .filter_map(|i| i.process.as_ref())
            .any(ProcessIdentity::is_running)
    }

    /// What a restore would rebuild, without ids, timestamps, or titles, so two
    /// snapshots of an unchanged kitty compare equal.
    pub fn fingerprint(&self) -> Value {
        let tabs: Vec<Value> = self
            .tabs()
            .map(|t| {
                let windows: Vec<Value> = t
                    .windows
                    .iter()
                    .map(|w| {
                        json!({
                            "cwd": w.cwd,
                            "agent": w.agent.as_ref().map(|a| (&a.kind, &a.session_id)),
                        })
                    })
                    .collect();
                json!({
                    "name": t.name,
                    "layout": t.layout,
                    "pairs": t.layout_state.get("pairs"),
                    "windows": windows,
                })
            })
            .collect();
        Value::Array(tabs)
    }
}

impl Instance {
    fn capture(client: &Client) -> anyhow::Result<Instance> {
        let raw = client.ls()?;
        let parsed = kitty::ls::parse(&raw)?;
        // kitty is the parent of the process it starts in each window.
        let process = parsed
            .iter()
            .flat_map(|o| &o.tabs)
            .flat_map(|t| &t.windows)
            .find_map(|w| procfs::ppid(w.pid))
            .and_then(ProcessIdentity::of);
        let os_windows = parsed.into_iter().map(OsWindow::from_ls).collect();
        Ok(Instance {
            socket: client.addr.to_string(),
            process,
            os_windows,
            raw,
        })
    }
}

impl OsWindow {
    fn from_ls(o: kitty::ls::OsWindow) -> OsWindow {
        OsWindow {
            id: o.id,
            is_active: o.is_active || o.is_focused,
            wm_class: o.wm_class,
            tabs: o.tabs.into_iter().map(Tab::from_ls).collect(),
        }
    }
}

impl Tab {
    fn from_ls(t: kitty::ls::Tab) -> Tab {
        let windows: Vec<Window> = t.windows.into_iter().map(Window::from_ls).collect();
        let name = windows
            .iter()
            .find_map(|w| w.user_vars.get(TAB_NAME_VAR)?.as_str().map(str::to_string))
            .filter(|n| !n.is_empty());
        Tab {
            id: t.id,
            title: t.title,
            name,
            is_active: t.is_active,
            layout: t.layout,
            layout_state: t.layout_state,
            windows,
        }
    }

    /// The window that had focus in this tab.
    pub fn focused(&self) -> Option<&Window> {
        self.windows.iter().find(|w| w.is_focused).or(self.windows.first())
    }
}

impl Window {
    fn from_ls(w: kitty::ls::Window) -> Window {
        let foreground: Vec<Process> = w
            .foreground_processes
            .into_iter()
            .map(|p| Process {
                pid: p.pid,
                cmdline: p.cmdline,
                cwd: p.cwd,
            })
            .collect();
        let agent = foreground.iter().find_map(|p| agent::detect(p.pid, &p.cmdline));
        // An agent's own directory is where it must be resumed; the window's
        // can differ if the shell moved since.
        let cwd = agent
            .as_ref()
            .and_then(|a| foreground.iter().find(|p| p.pid == a.pid))
            .map(|p| p.cwd.clone())
            .filter(|c| !c.is_empty())
            .unwrap_or(w.cwd);
        Window {
            id: w.id,
            title: w.title,
            cwd,
            cmdline: w.cmdline,
            is_focused: w.is_focused || w.is_active,
            foreground,
            user_vars: w.user_vars,
            agent,
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A snapshot built from a canned `ls`, as kitty reports it. Paths and ids
    /// are made up.
    pub fn fixture() -> Snapshot {
        let raw: Value = serde_json::from_str(include_str!("../tests/fixtures/ls.json")).unwrap();
        let os_windows = kitty::ls::parse(&raw)
            .unwrap()
            .into_iter()
            .map(OsWindow::from_ls)
            .collect();
        Snapshot {
            format: FORMAT,
            kitmux_version: "0.0.0".into(),
            taken_at: "2026-01-02T03:04:05Z".parse().unwrap(),
            hostname: Some("host".into()),
            reason: "manual".into(),
            instances: vec![Instance {
                socket: "unix:/tmp/kitty-100".into(),
                process: None,
                os_windows,
                raw,
            }],
        }
    }

    #[test]
    fn reads_kittys_ls() {
        let s = fixture();
        let tabs: Vec<&Tab> = s.tabs().collect();
        assert_eq!(tabs.len(), 3);
        assert_eq!(tabs[0].layout, "fat");
        assert_eq!(tabs[1].name.as_deref(), Some("api"));
        assert!(tabs[1].is_active);
        assert_eq!(tabs[2].layout, "splits");
        assert_eq!(s.windows().count(), 6);
        let w = &tabs[1].windows[0];
        assert_eq!(w.cwd, "/home/user/src/api");
        assert!(w.is_focused);
    }

    #[test]
    fn fingerprint_ignores_titles_and_ids() {
        let a = fixture();
        let mut b = fixture();
        b.taken_at = Utc::now();
        b.instances[0].os_windows[0].tabs[0].title = "something else".into();
        b.instances[0].os_windows[0].tabs[0].windows[0].id = 999;
        assert_eq!(a.fingerprint(), b.fingerprint());

        b.instances[0].os_windows[0].tabs[0].windows[0].cwd = "/elsewhere".into();
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn round_trips_through_json() {
        let s = fixture();
        let back: Snapshot = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.fingerprint(), s.fingerprint());
        assert_eq!(back.instances[0].raw, s.instances[0].raw);
    }
}
