//! `kit tab`: change tabs in a way kit remembers.

use anyhow::{Result, anyhow, bail};
use serde_json::json;

use crate::kitty::{Addr, Client, discover, ls};
use crate::snapshot::TAB_NAME_VAR;

/// Names a tab. The name becomes the tab's title and is recorded as a user
/// variable on its windows, so `kit save` knows it was chosen rather than set
/// by whatever ran in the tab, and `kit resume` gives it back. An empty name
/// clears it.
pub fn set_name(socket: Option<&str>, tab: Option<u64>, name: &str) -> Result<()> {
    let client = client(socket)?;
    let tree = ls::parse(&client.ls()?)?;
    let here: Option<u64> = std::env::var("KITTY_WINDOW_ID").ok().and_then(|v| v.parse().ok());
    let all = || tree.iter().flat_map(|o| &o.tabs);
    let target = match tab {
        Some(id) => all().find(|t| t.id == id),
        None => here
            .and_then(|w| all().find(|t| t.windows.iter().any(|x| x.id == w)))
            .or_else(|| {
                tree.iter()
                    .find(|o| o.is_focused)
                    .or(tree.first())
                    .and_then(|o| o.tabs.iter().find(|t| t.is_active))
            }),
    }
    .ok_or_else(|| anyhow!("no such tab"))?;

    client.call("set-tab-title", json!({ "title": name, "match": format!("id:{}", target.id) }))?;
    let var = if name.is_empty() { TAB_NAME_VAR.to_string() } else { format!("{TAB_NAME_VAR}={name}") };
    for w in &target.windows {
        client.call("set-user-vars", json!({ "var": [var], "match": format!("id:{}", w.id) }))?;
    }
    if name.is_empty() {
        println!("tab {} unnamed", target.id);
    } else {
        println!("tab {} named {name:?}", target.id);
    }
    Ok(())
}

/// The kitty to act on: the one kit runs in, else the only one running.
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

