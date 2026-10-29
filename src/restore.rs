//! Rebuilding a snapshot in kitty.
//!
//! Restoring is split in two: [`plan`] turns a snapshot into the windows to
//! create and where each goes, without touching kitty, and [`execute`] carries
//! a plan out over remote control. The plan is what `kitmux resume --dry-run`
//! prints and what the tests check.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use serde_json::{Value, json};

use crate::agent::{self, ResumeOptions};
use crate::kitty::Client;
use crate::snapshot::{self, Snapshot, Tab};

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Resume the agent sessions; otherwise every window gets a plain shell.
    pub agents: bool,
    pub resume: ResumeOptions,
    /// Restore tabs whose agent sessions are already running somewhere,
    /// giving those windows a plain shell instead of a second copy.
    pub force: bool,
}

#[derive(Debug, PartialEq)]
pub struct Plan {
    pub os_windows: Vec<OsWindowPlan>,
    /// Tabs left out, with why.
    pub skipped: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub struct OsWindowPlan {
    pub tabs: Vec<TabPlan>,
    /// Index into `tabs` of the tab to leave active.
    pub active_tab: usize,
    pub is_active: bool,
}

#[derive(Debug, PartialEq)]
pub struct TabPlan {
    pub layout: String,
    /// Set only for a tab named with `kitmux tab set --name`.
    pub title: Option<String>,
    /// In creation order; `Place` refers to earlier entries by index.
    pub windows: Vec<WindowPlan>,
    /// Index into `windows` of the window to focus.
    pub focus: usize,
}

#[derive(Debug, PartialEq)]
pub struct WindowPlan {
    pub cwd: String,
    /// What to run; None is kitty's default shell.
    pub args: Option<Vec<String>>,
    pub env: Vec<String>,
    /// kitty user variables to set, as `name=value`.
    pub vars: Vec<String>,
    pub place: Place,
    /// The agent session this window resumes, as `kind id`.
    pub resumes: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Place {
    /// The tab's first window.
    First,
    /// After an earlier window, in the layout's own order.
    After(usize),
    /// Splitting an earlier window, side by side (`vertical`) or stacked.
    /// `bias` is the new window's share of the space, in percent.
    Split { of: usize, vertical: bool, bias: u8 },
}

/// What restoring `snap` would create. `running` is the agent sessions running
/// now, as `(kind, id)`; tabs holding one are left out unless forced.
pub fn plan(snap: &Snapshot, running: &HashSet<(String, String)>, opts: Options) -> Plan {
    let mut out = Plan {
        os_windows: Vec::new(),
        skipped: Vec::new(),
    };
    for inst in &snap.instances {
        for o in &inst.os_windows {
            let mut tabs = Vec::new();
            let mut active_tab = 0;
            for t in &o.tabs {
                let live: Vec<String> = t
                    .windows
                    .iter()
                    .filter_map(|w| w.agent.as_ref())
                    .filter(|a| running.contains(&(a.kind.clone(), a.session_id.clone())))
                    .map(|a| format!("{} {}", a.kind, a.session_id))
                    .collect();
                if !live.is_empty() && !opts.force {
                    out.skipped.push(format!(
                        "tab {:?}: {} already running (--force restores it with a shell in its place)",
                        t.name.as_deref().unwrap_or(&t.title),
                        live.join(", ")
                    ));
                    continue;
                }
                if t.is_active {
                    active_tab = tabs.len();
                }
                tabs.push(plan_tab(t, running, opts));
            }
            if !tabs.is_empty() {
                out.os_windows.push(OsWindowPlan {
                    tabs,
                    active_tab,
                    is_active: o.is_active,
                });
            }
        }
    }
    out
}

fn plan_tab(t: &Tab, running: &HashSet<(String, String)>, opts: Options) -> TabPlan {
    let order = window_order(t);
    let focused_id = t.focused().map(|w| w.id);
    let vars: Vec<String> = t
        .name
        .iter()
        .map(|n| format!("{}={n}", snapshot::TAB_NAME_VAR))
        .collect();
    let mut windows = Vec::new();
    let mut focus = 0;
    for (idx, place) in order {
        let w = &t.windows[idx];
        if Some(w.id) == focused_id {
            focus = windows.len();
        }
        let agent = w
            .agent
            .as_ref()
            .filter(|_| opts.agents)
            .filter(|a| !running.contains(&(a.kind.clone(), a.session_id.clone())));
        let (args, env, resumes) = match agent {
            Some(a) => {
                let shell = shell_for(&w.cmdline);
                let cmd = crate::cmd::shell_join(&a.resume_command(opts.resume));
                let script = format!("{cmd}; exec {}", crate::cmd::shell_join(std::slice::from_ref(&shell)));
                (
                    Some(vec![shell, "-ic".to_string(), script]),
                    a.env.clone(),
                    Some(format!("{} {}", a.kind, a.session_id)),
                )
            }
            None => (None, Vec::new(), None),
        };
        windows.push(WindowPlan {
            cwd: w.cwd.clone(),
            args,
            env,
            vars: vars.clone(),
            place,
            resumes,
        });
    }
    TabPlan {
        layout: t.layout.clone(),
        title: t.name.clone(),
        windows,
        focus,
    }
}

/// The shell to run an agent under: the window's own if it was started with a
/// known shell, else `$SHELL`. A login shell's leading `-` is dropped.
fn shell_for(cmdline: &[String]) -> String {
    const SHELLS: &[&str] = &["bash", "zsh", "fish", "sh", "dash", "ksh"];
    let own = cmdline
        .first()
        .filter(|c| SHELLS.contains(&c.rsplit('/').next().unwrap_or(c).trim_start_matches('-')));
    match own {
        Some(s) => s.trim_start_matches('-').to_string(),
        None => std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
    }
}

/// The order to create a tab's windows in, as indexes into `t.windows`, with
/// where each goes. For a splits layout this rebuilds kitty's split tree;
/// other layouts arrange windows by order alone.
fn window_order(t: &Tab) -> Vec<(usize, Place)> {
    let index: HashMap<u64, usize> = t.windows.iter().enumerate().map(|(i, w)| (w.id, i)).collect();
    let mut out = Vec::new();
    if t.layout == "splits"
        && let Some(pairs) = t.layout_state.get("pairs")
    {
        let groups = group_windows(&t.layout_state);
        let leaf = |g: u64| groups.get(&g).copied().unwrap_or(g);
        if let Some(tree) = Node::parse(pairs, &leaf, &index) {
            let first = tree.first();
            out.push((first, Place::First));
            tree.emit(first, &mut out);
        }
    }
    // Everything not placed by a split tree goes after the last window.
    let placed: HashSet<usize> = out.iter().map(|(i, _)| *i).collect();
    for i in 0..t.windows.len() {
        if placed.contains(&i) {
            continue;
        }
        let place = match out.last() {
            None => Place::First,
            Some((prev, _)) => Place::After(*prev),
        };
        out.push((i, place));
    }
    out
}

/// kitty's split tree names window groups; a group's first window stands for it.
fn group_windows(layout_state: &Value) -> HashMap<u64, u64> {
    let mut out = HashMap::new();
    let groups = layout_state
        .pointer("/all_windows/window_groups")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for g in groups {
        let id = g.get("id").and_then(Value::as_u64);
        let first = g.pointer("/window_ids/0").and_then(Value::as_u64);
        if let (Some(id), Some(first)) = (id, first) {
            out.insert(id, first);
        }
    }
    out
}

/// A splits layout's tree, with leaves resolved to window indexes.
enum Node {
    Leaf(usize),
    Pair {
        one: Box<Node>,
        two: Box<Node>,
        horizontal: bool,
        bias: f64,
    },
}

impl Node {
    /// Parses kitty's `pairs`, dropping leaves for windows that are gone.
    fn parse(v: &Value, leaf: &dyn Fn(u64) -> u64, index: &HashMap<u64, usize>) -> Option<Node> {
        if let Some(g) = v.as_u64() {
            return index.get(&leaf(g)).map(|i| Node::Leaf(*i));
        }
        let one = v.get("one").and_then(|x| Node::parse(x, leaf, index));
        let two = v.get("two").and_then(|x| Node::parse(x, leaf, index));
        match (one, two) {
            (Some(one), Some(two)) => Some(Node::Pair {
                one: Box::new(one),
                two: Box::new(two),
                // kitty omits these when they are the defaults.
                horizontal: v.get("horizontal").and_then(Value::as_bool).unwrap_or(true),
                bias: v.get("bias").and_then(Value::as_f64).unwrap_or(0.5),
            }),
            (Some(n), None) | (None, Some(n)) => Some(n),
            (None, None) => None,
        }
    }

    fn first(&self) -> usize {
        match self {
            Node::Leaf(i) => *i,
            Node::Pair { one, .. } => one.first(),
        }
    }

    /// Emits the windows of this subtree after its first, which already
    /// exists. Splitting the first window of `one` makes the pair; each side
    /// then grows from its own first window.
    fn emit(&self, first: usize, out: &mut Vec<(usize, Place)>) {
        if let Node::Pair {
            one,
            two,
            horizontal,
            bias,
        } = self
        {
            let second = two.first();
            let share = ((1.0 - bias) * 100.0).round().clamp(1.0, 99.0) as u8;
            out.push((
                second,
                Place::Split {
                    of: first,
                    vertical: *horizontal,
                    bias: share,
                },
            ));
            one.emit(first, out);
            two.emit(second, out);
        }
    }
}

/// Where restored tabs go.
pub struct Target {
    pub client: Client,
    /// A window in the OS window that gets the snapshot's first OS window, if
    /// not whichever is active.
    pub window: Option<u64>,
}

/// Creates what `plan` describes. Returns the new windows' ids.
pub fn execute(plan: &Plan, target: &Target) -> Result<Vec<u64>> {
    let c = &target.client;
    let mut created = Vec::new();
    // Focus the snapshot's active OS window last, so it ends up on top.
    let mut order: Vec<usize> = (0..plan.os_windows.len()).collect();
    order.sort_by_key(|i| plan.os_windows[*i].is_active);

    for oi in order {
        let o = &plan.os_windows[oi];
        let mut anchor: Option<u64> = None;
        // The first OS window's tabs join the target's; others get their own.
        let into_target = oi == 0;
        let mut focus_ids = Vec::new();
        for (ti, tab) in o.tabs.iter().enumerate() {
            let mut ids: Vec<u64> = Vec::new();
            for (wi, w) in tab.windows.iter().enumerate() {
                let mut p = json!({ "cwd": w.cwd });
                if let Some(args) = &w.args {
                    p["args"] = json!(args);
                }
                if !w.env.is_empty() {
                    p["env"] = json!(w.env);
                }
                if !w.vars.is_empty() {
                    p["var"] = json!(w.vars);
                }
                match &w.place {
                    Place::First => {
                        match anchor {
                            // Further tabs go into the OS window the first made.
                            Some(a) => {
                                p["type"] = json!("tab");
                                p["match"] = json!(format!("window_id:{a}"));
                            }
                            None if into_target => {
                                p["type"] = json!("tab");
                                if let Some(win) = target.window {
                                    p["match"] = json!(format!("window_id:{win}"));
                                }
                            }
                            None => p["type"] = json!("os-window"),
                        }
                        if let Some(title) = &tab.title {
                            p["tab_title"] = json!(title);
                        }
                    }
                    Place::After(i) => {
                        p["type"] = json!("window");
                        p["next_to"] = json!(format!("id:{}", ids[*i]));
                        p["location"] = json!("after");
                    }
                    Place::Split { of, vertical, bias } => {
                        p["type"] = json!("window");
                        p["next_to"] = json!(format!("id:{}", ids[*of]));
                        p["location"] = json!(if *vertical { "vsplit" } else { "hsplit" });
                        p["bias"] = json!(bias);
                    }
                }
                let id = c.launch(p)?;
                if wi == 0 {
                    anchor.get_or_insert(id);
                    // The layout must be in place before windows join it, or
                    // splits would be laid out by another layout's rules.
                    if let Err(e) = c.call(
                        "goto-layout",
                        json!({ "layout": tab.layout, "match": format!("window_id:{id}") }),
                    ) {
                        eprintln!("kitmux: keeping kitty's default layout for a tab: {e:#}");
                    }
                }
                ids.push(id);
                created.push(id);
            }
            if let Some(id) = ids.get(tab.focus) {
                focus_ids.push((ti == o.active_tab, *id));
            }
        }
        // Each tab's focused window, then the active tab's last.
        focus_ids.sort_by_key(|(active, _)| *active);
        for (_, id) in focus_ids {
            let _ = c.call("focus-window", json!({ "match": format!("id:{id}") }));
        }
    }
    Ok(created)
}

/// The agent sessions running on this machine now.
pub fn running_sessions() -> HashSet<(String, String)> {
    agent::running().into_iter().map(|s| (s.kind, s.session_id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::tests::fixture;

    const OPTS: Options = Options {
        agents: true,
        resume: ResumeOptions { continue_turn: false },
        force: false,
    };

    fn with_agents() -> Snapshot {
        let mut s = fixture();
        let tabs = &mut s.instances[0].os_windows[0].tabs;
        tabs[1].windows[0].agent = Some(agent::Session {
            kind: "claude".into(),
            session_id: "c-1".into(),
            pid: 1,
            cmdline: vec!["claude".into(), "--model".into(), "opus".into()],
            env: vec!["CLAUDE_CONFIG_DIR=/home/user/.claude-work".into()],
            resume: vec![],
        });
        tabs[2].windows[0].agent = Some(agent::Session {
            kind: "omp".into(),
            session_id: "o-1".into(),
            pid: 2,
            cmdline: vec!["omp".into()],
            env: vec![],
            resume: vec![],
        });
        s
    }

    #[test]
    fn plans_every_tab_and_window() {
        let p = plan(&with_agents(), &HashSet::new(), OPTS);
        assert!(p.skipped.is_empty());
        assert_eq!(p.os_windows.len(), 1);
        let o = &p.os_windows[0];
        assert_eq!(o.tabs.len(), 3);
        assert_eq!(o.active_tab, 1);
        assert_eq!(
            o.tabs.iter().map(|t| t.layout.as_str()).collect::<Vec<_>>(),
            ["fat", "grid", "splits"]
        );

        // Only a tab named with kitmux gets a title.
        assert_eq!(o.tabs[0].title, None);
        assert_eq!(o.tabs[1].title.as_deref(), Some("api"));
        assert_eq!(o.tabs[1].windows[1].vars, ["kitmux_tab_name=api"]);
    }

    #[test]
    fn resumes_agents_under_a_shell() {
        let p = plan(&with_agents(), &HashSet::new(), OPTS);
        let w = &p.os_windows[0].tabs[1].windows[0];
        assert_eq!(w.resumes.as_deref(), Some("claude c-1"));
        assert_eq!(
            w.args.as_deref().unwrap(),
            ["/bin/zsh", "-ic", "claude --model opus --resume c-1; exec /bin/zsh"]
        );
        assert_eq!(w.env, ["CLAUDE_CONFIG_DIR=/home/user/.claude-work"]);
        // A window with no agent gets the default shell.
        assert_eq!(p.os_windows[0].tabs[1].windows[1].args, None);

        let restore = Options {
            resume: ResumeOptions { continue_turn: true },
            ..OPTS
        };
        let p = plan(&with_agents(), &HashSet::new(), restore);
        let w = &p.os_windows[0].tabs[2].windows[0];
        assert_eq!(w.args.as_deref().unwrap()[2], "omp --restore o-1; exec /bin/zsh");
    }

    #[test]
    fn without_agents_every_window_is_a_shell() {
        let p = plan(&with_agents(), &HashSet::new(), Options { agents: false, ..OPTS });
        let all: Vec<_> = p.os_windows[0].tabs.iter().flat_map(|t| &t.windows).collect();
        assert_eq!(all.len(), 6);
        assert!(all.iter().all(|w| w.args.is_none() && w.resumes.is_none()));
    }

    #[test]
    fn skips_tabs_whose_agents_are_running() {
        let running: HashSet<_> = [("claude".to_string(), "c-1".to_string())].into();
        let p = plan(&with_agents(), &running, OPTS);
        assert_eq!(p.os_windows[0].tabs.len(), 2);
        assert_eq!(p.skipped.len(), 1);
        assert!(p.skipped[0].contains("claude c-1"), "{}", p.skipped[0]);

        let p = plan(&with_agents(), &running, Options { force: true, ..OPTS });
        let tab = &p.os_windows[0].tabs[1];
        assert_eq!(tab.windows[0].args, None, "a running session is never started twice");
    }

    #[test]
    fn rebuilds_the_split_tree() {
        let p = plan(&with_agents(), &HashSet::new(), OPTS);
        let tab = &p.os_windows[0].tabs[2];
        // pairs: { one: 4, two: { horizontal: false, bias: 0.7, one: 5, two: 6 } }
        let places: Vec<_> = tab.windows.iter().map(|w| (w.cwd.as_str(), w.place.clone())).collect();
        assert_eq!(
            places,
            [
                ("/home/user/src/web", Place::First),
                (
                    "/home/user/src/web",
                    Place::Split {
                        of: 0,
                        vertical: true,
                        bias: 50
                    }
                ),
                (
                    "/home/user/src/web/docs",
                    Place::Split {
                        of: 1,
                        vertical: false,
                        bias: 30
                    }
                ),
            ]
        );
        assert_eq!(tab.focus, 2);
    }

    #[test]
    fn other_layouts_keep_window_order() {
        let p = plan(&with_agents(), &HashSet::new(), OPTS);
        let tab = &p.os_windows[0].tabs[1];
        assert_eq!(tab.windows[0].place, Place::First);
        assert_eq!(tab.windows[1].place, Place::After(0));
        assert_eq!(tab.focus, 0);
    }

    #[test]
    fn nested_splits_grow_from_each_side() {
        // A 2x2 grid of splits: columns first, then each column split in two.
        let mut s = fixture();
        let tab = &mut s.instances[0].os_windows[0].tabs[2];
        let mut extra = tab.windows[0].clone();
        extra.id = 7;
        tab.windows.push(extra);
        tab.layout_state["pairs"] = json!({
            "one": { "horizontal": false, "one": 4, "two": 5 },
            "two": { "horizontal": false, "one": 6, "two": 7 },
        });
        let order = window_order(tab);
        assert_eq!(
            order,
            [
                (0, Place::First),
                (
                    2,
                    Place::Split {
                        of: 0,
                        vertical: true,
                        bias: 50
                    }
                ),
                (
                    1,
                    Place::Split {
                        of: 0,
                        vertical: false,
                        bias: 50
                    }
                ),
                (
                    3,
                    Place::Split {
                        of: 2,
                        vertical: false,
                        bias: 50
                    }
                ),
            ]
        );
    }
}
