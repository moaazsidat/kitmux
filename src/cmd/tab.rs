//! `kitmux tab`: change tabs in a way kitmux remembers.

use std::io::{IsTerminal, Write};

use anyhow::{Result, anyhow, bail};
use serde_json::json;

use super::tilde;
use crate::kitty::{Addr, Client, discover, ls};
use crate::snapshot::TAB_NAME_VAR;

/// Names a tab. The name becomes the tab's title and is recorded as a user
/// variable on its windows, so `kitmux save` knows it was chosen rather than set
/// by whatever ran in the tab, and `kitmux resume` gives it back. An empty name
/// clears it; no name asks for one.
pub fn name(socket: Option<&str>, tab: Option<&str>, name: Option<String>) -> Result<()> {
    let client = client(socket)?;
    let tree = ls::parse(&client.ls()?)?;
    let here: Option<u64> = std::env::var("KITTY_WINDOW_ID").ok().and_then(|v| v.parse().ok());
    let target = find(&tree, tab, here)?;

    let name = match name {
        Some(name) => name,
        None => match ask(&current(target))? {
            Some(name) => name,
            None => return Ok(()),
        },
    };

    client.call(
        "set-tab-title",
        json!({ "title": name, "match": format!("id:{}", target.id) }),
    )?;
    let var = if name.is_empty() {
        TAB_NAME_VAR.to_string()
    } else {
        format!("{TAB_NAME_VAR}={name}")
    };
    for w in &target.windows {
        client.call(
            "set-user-vars",
            json!({ "var": [var], "match": format!("id:{}", w.id) }),
        )?;
    }
    if name.is_empty() {
        println!("tab {} unnamed", target.id);
    } else {
        println!("tab {} named {name:?}", target.id);
    }
    Ok(())
}

/// Lists the open tabs by position, with their names.
pub fn ls(socket: Option<&str>) -> Result<()> {
    let client = client(socket)?;
    let tree = ls::parse(&client.ls()?)?;
    for (i, os) in tree.iter().enumerate() {
        if tree.len() > 1 {
            println!("os window {}", i + 1);
        }
        for (pos, t) in os.tabs.iter().enumerate() {
            let mark = if t.is_active { '*' } else { ' ' };
            let name = match named(t) {
                Some(n) => format!("{n:?}"),
                None => format!("({})", t.title),
            };
            let cwd = t
                .windows
                .iter()
                .find(|w| w.is_focused)
                .or(t.windows.first())
                .map(|w| tilde(&w.cwd))
                .unwrap_or_default();
            println!("{mark} {:>2}  id:{:<4} {name}  {cwd}", pos + 1, t.id);
        }
    }
    Ok(())
}

/// The tab `spec` picks: a position in its OS window or `id:N`. Without one,
/// the tab holding window `here`, else the focused OS window's active tab.
fn find<'a>(tree: &'a [ls::OsWindow], spec: Option<&str>, here: Option<u64>) -> Result<&'a ls::Tab> {
    let all = || tree.iter().flat_map(|o| &o.tabs);
    let home = || {
        here.and_then(|w| {
            tree.iter()
                .find(|o| o.tabs.iter().any(|t| t.windows.iter().any(|x| x.id == w)))
        })
        .or_else(|| tree.iter().find(|o| o.is_focused))
        .or(tree.first())
    };
    let tab = match spec {
        Some(spec) => {
            if let Some(id) = spec.strip_prefix("id:") {
                let id: u64 = id.parse().map_err(|_| anyhow!("bad tab id {id:?}"))?;
                all().find(|t| t.id == id)
            } else {
                let pos: usize = spec
                    .parse()
                    .ok()
                    .filter(|&p| p > 0)
                    .ok_or_else(|| anyhow!("--tab takes a position like 2 or an id like id:7, not {spec:?}"))?;
                home().and_then(|o| o.tabs.get(pos - 1))
            }
        }
        None => here
            .and_then(|w| all().find(|t| t.windows.iter().any(|x| x.id == w)))
            .or_else(|| home().and_then(|o| o.tabs.iter().find(|t| t.is_active))),
    };
    tab.ok_or_else(|| anyhow!("no such tab"))
}

/// The name kitmux gave the tab, if any.
fn named(t: &ls::Tab) -> Option<&str> {
    t.windows
        .iter()
        .find_map(|w| w.user_vars.get(TAB_NAME_VAR).and_then(|v| v.as_str()))
}

fn current(t: &ls::Tab) -> String {
    named(t).unwrap_or(&t.title).to_string()
}

/// Asks for a name on the terminal. Nothing typed keeps the tab as it is.
fn ask(current: &str) -> Result<Option<String>> {
    if !std::io::stdin().is_terminal() {
        bail!("give a name, or run in a terminal to be asked for one");
    }
    eprint!("name [{current}]: ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let line = line.trim();
    Ok((!line.is_empty()).then(|| line.to_string()))
}

/// The kitty to act on: the one kitmux runs in, else the only one running.
fn client(socket: Option<&str>) -> Result<Client> {
    let clients = discover::find(socket)?;
    let here = std::env::var("KITTY_LISTEN_ON").ok().and_then(|v| Addr::parse(&v).ok());
    if let Some(c) = here.and_then(|h| clients.iter().find(|c| c.addr == h).cloned()) {
        return Ok(c);
    }
    match clients.len() {
        0 => bail!("no kitty is reachable over remote control"),
        1 => Ok(clients[0].clone()),
        _ => bail!("more than one kitty is running; choose one with --socket"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Vec<ls::OsWindow> {
        ls::parse(&serde_json::from_str(include_str!("../../tests/fixtures/ls.json")).unwrap()).unwrap()
    }

    #[test]
    fn finds_tabs() {
        let tree = tree();
        let id = |spec, here| find(&tree, spec, here).map(|t| t.id).ok();
        assert_eq!(id(None, None), Some(2));
        assert_eq!(id(None, Some(5)), Some(3));
        assert_eq!(id(Some("1"), None), Some(1));
        assert_eq!(id(Some("3"), Some(1)), Some(3));
        assert_eq!(id(Some("id:3"), None), Some(3));
        assert_eq!(id(Some("4"), None), None);
        assert_eq!(id(Some("0"), None), None);
        assert_eq!(id(Some("api"), None), None);
    }

    #[test]
    fn reads_names() {
        let tree = tree();
        let names: Vec<_> = tree[0].tabs.iter().map(named).collect();
        assert_eq!(names, [None, Some("api"), None]);
    }
}
