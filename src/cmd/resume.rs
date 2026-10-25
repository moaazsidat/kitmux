//! `kit resume`: rebuild a snapshot's tabs and resume its agent sessions.

use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::json;

use super::{shell_join, tilde};
use crate::kitty::{Addr, Client, discover};
use crate::restore::{self, Place, Plan, Target};
use crate::store::{Entry, Store};

pub struct Options {
    pub snapshot: Option<String>,
    pub restore: restore::Options,
    pub dry_run: bool,
}

pub fn run(socket: Option<&str>, opts: Options) -> Result<()> {
    let entry = pick(opts.snapshot.as_deref())?;
    let running = restore::running_sessions();
    let plan = restore::plan(&entry.snapshot, &running, opts.restore);

    println!("from {}", entry.path.display());
    for s in &plan.skipped {
        println!("  skipping {s}");
    }
    if plan.os_windows.is_empty() {
        println!("nothing to restore");
        return Ok(());
    }
    if opts.dry_run {
        print_plan(&plan);
        return Ok(());
    }

    let (target, bootstrap) = target(socket)?;
    restore::execute(&plan, &target)?;
    // A kitty kit started opens with a shell nobody asked for.
    for id in bootstrap {
        let _ = target
            .client
            .call("close-window", json!({ "match": format!("id:{id}") }));
    }

    let tabs: usize = plan.os_windows.iter().map(|o| o.tabs.len()).sum();
    let agents = plan
        .os_windows
        .iter()
        .flat_map(|o| &o.tabs)
        .flat_map(|t| &t.windows)
        .filter(|w| w.resumes.is_some())
        .count();
    println!("restored {tabs} tabs and resumed {agents} agent sessions");
    Ok(())
}

/// The snapshot to restore. By default, the newest one taken of a kitty that
/// is no longer running: after a crash or restart, snapshots of the new kitty
/// (often a single empty tab) are not what anyone wants back.
fn pick(reference: Option<&str>) -> Result<Entry> {
    let store = Store::open_default()?;
    if let Some(r) = reference {
        return store.resolve(r);
    }
    let mut all = store.list()?.into_iter();
    let Some(newest) = all.next() else {
        bail!("no snapshots yet in {} — run `kit save` first", store.dir().display());
    };
    if !newest.snapshot.of_running_kitty() {
        return Ok(newest);
    }
    match all.find(|e| !e.snapshot.of_running_kitty()) {
        Some(e) => {
            println!(
                "(newest snapshot is of the kitty running now; using the last one before it — `kit resume latest` for the newest)"
            );
            Ok(e)
        }
        None => Ok(newest),
    }
}

/// The kitty to restore into, and windows to close once done. With no kitty
/// running, one is started.
fn target(socket: Option<&str>) -> Result<(Target, Vec<u64>)> {
    let clients = discover::find(socket)?;
    let here = std::env::var("KITTY_LISTEN_ON").ok().and_then(|v| Addr::parse(&v).ok());
    let window = std::env::var("KITTY_WINDOW_ID").ok().and_then(|v| v.parse().ok());

    if let Some(c) = here.as_ref().and_then(|h| clients.iter().find(|c| &c.addr == h)) {
        // Run from inside kitty: tabs go next to the window kit was run in.
        return Ok((
            Target {
                client: c.clone(),
                window,
            },
            Vec::new(),
        ));
    }
    match clients.len() {
        0 => {
            let (client, bootstrap) = spawn_kitty()?;
            Ok((Target { client, window: None }, bootstrap))
        }
        1 => Ok((
            Target {
                client: clients[0].clone(),
                window: None,
            },
            Vec::new(),
        )),
        _ => {
            let list: Vec<String> = clients.iter().map(|c| c.addr.to_string()).collect();
            bail!(
                "more than one kitty is running; choose one with --socket: {}",
                list.join(", ")
            )
        }
    }
}

/// Starts kitty with remote control on, and waits for it to answer.
fn spawn_kitty() -> Result<(Client, Vec<u64>)> {
    let mut cmd = Command::new("kitty");
    cmd.arg("--detach");
    let remote_ok = discover::conf_option("allow_remote_control")
        .is_some_and(|v| matches!(v.as_str(), "yes" | "y" | "true" | "socket-only" | "socket"));
    if !remote_ok {
        cmd.args(["-o", "allow_remote_control=socket-only"]);
    }
    let own_socket = if discover::conf_option("listen_on").is_some_and(|v| v != "none") {
        None
    } else {
        let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let addr = format!("unix:{dir}/kitty-kit-{}", std::process::id());
        cmd.args(["--listen-on", &addr]);
        Some(addr)
    };
    println!("starting kitty");
    cmd.status().context("starting kitty (is it on PATH?)")?;

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let found = discover::find(own_socket.as_deref()).unwrap_or_default();
        let ready = found.into_iter().find(|c| c.ls().is_ok());
        if let Some(c) = ready {
            let bootstrap = c
                .ls()?
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|o| o["tabs"].as_array().cloned().unwrap_or_default())
                .flat_map(|t| t["windows"].as_array().cloned().unwrap_or_default())
                .filter_map(|w| w["id"].as_u64())
                .collect();
            return Ok((c, bootstrap));
        }
        if Instant::now() > deadline {
            bail!("started kitty, but it did not answer remote control within 10s");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn print_plan(plan: &Plan) {
    for (oi, o) in plan.os_windows.iter().enumerate() {
        println!("os window {}", oi + 1);
        for (ti, t) in o.tabs.iter().enumerate() {
            let active = if ti == o.active_tab { " *" } else { "" };
            let title = t.title.as_deref().map(|n| format!(" {n:?}")).unwrap_or_default();
            println!("  tab {}{active} [{}]{title}", ti + 1, t.layout);
            for (wi, w) in t.windows.iter().enumerate() {
                let place = match &w.place {
                    Place::First => String::new(),
                    Place::After(i) => format!("after {}", i + 1),
                    Place::Split { of, vertical, bias } => {
                        format!("{} {} ({bias}%)", if *vertical { "beside" } else { "below" }, of + 1)
                    }
                };
                let what = w
                    .args
                    .as_ref()
                    .map(|a| shell_join(a))
                    .unwrap_or_else(|| "shell".to_string());
                let focus = if wi == t.focus { "*" } else { " " };
                println!("    {focus}{} {:<14} {:<28} {what}", wi + 1, place, tilde(&w.cwd));
            }
        }
    }
}
