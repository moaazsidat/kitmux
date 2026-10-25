//! `kit ls` and `kit show`: what has been saved.

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{Local, Utc};

use super::{shell_join, tilde};
use crate::agent::ResumeOptions;
use crate::snapshot::Snapshot;
use crate::store::{Entry, Store};

pub fn list(json: bool) -> Result<()> {
    let store = Store::open_default()?;
    let entries = store.list()?;
    if json {
        let rows: Vec<_> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "path": e.path,
                    "taken_at": e.snapshot.taken_at,
                    "reason": e.snapshot.reason,
                    "tabs": e.snapshot.tabs().count(),
                    "windows": e.snapshot.windows().count(),
                    "agents": e.snapshot.agents().collect::<Vec<_>>(),
                    "running": e.snapshot.of_running_kitty(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if entries.is_empty() {
        println!("no snapshots yet in {} — run `kit save`", store.dir().display());
        return Ok(());
    }
    println!("{:>3}  {:<16}  {:>5}  {:>4}  {:>7}  {:<24}  {:<6}", "#", "TAKEN", "AGE", "TABS", "WINDOWS", "AGENTS", "BY");
    for (i, e) in entries.iter().enumerate() {
        let s = &e.snapshot;
        let note = if s.of_running_kitty() { "  (this kitty)" } else { "" };
        println!(
            "{:>3}  {:<16}  {:>5}  {:>4}  {:>7}  {:<24}  {:<6}{note}",
            i + 1,
            s.taken_at.with_timezone(&Local).format("%Y-%m-%d %H:%M"),
            age(s),
            s.tabs().count(),
            s.windows().count(),
            agent_summary(s),
            s.reason,
        );
    }
    println!("\n{} snapshots in {}. `kit show <#>` for detail.", entries.len(), store.dir().display());
    Ok(())
}

pub fn show(reference: &str, json: bool) -> Result<()> {
    let entry = Store::open_default()?.resolve(reference)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&entry.snapshot)?);
        return Ok(());
    }
    print_tree(&entry);
    Ok(())
}

pub fn print_tree(entry: &Entry) {
    let s = &entry.snapshot;
    println!(
        "{} ({}, {} ago) {}",
        s.taken_at.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S"),
        s.reason,
        age(s),
        entry.path.display()
    );
    for inst in &s.instances {
        let running = inst.process.as_ref().is_some_and(|p| p.is_running());
        println!("kitty at {}{}", inst.socket, if running { " (running)" } else { "" });
        for (oi, o) in inst.os_windows.iter().enumerate() {
            println!("  os window {}{}", oi + 1, if o.is_active { " *" } else { "" });
            for (ti, t) in o.tabs.iter().enumerate() {
                let label = t.name.clone().unwrap_or_else(|| format!("{:?}", t.title));
                println!("    tab {}{} [{}] {label}", ti + 1, if t.is_active { " *" } else { "" }, t.layout);
                for w in &t.windows {
                    let focus = if w.is_focused { "*" } else { " " };
                    let what = match &w.agent {
                        Some(a) => format!("{} {} → {}", a.kind, a.session_id, shell_join(&a.resume_command(ResumeOptions::default()))),
                        None => w
                            .foreground
                            .last()
                            .map(|p| shell_join(&p.cmdline))
                            .unwrap_or_default(),
                    };
                    println!("      {focus} {:<32} {what}", tilde(&w.cwd));
                }
            }
        }
    }
}

fn agent_summary(s: &Snapshot) -> String {
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for a in s.agents() {
        *kinds.entry(&a.kind).or_default() += 1;
    }
    if kinds.is_empty() {
        return "-".to_string();
    }
    kinds.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(", ")
}

fn age(s: &Snapshot) -> String {
    let secs = (Utc::now() - s.taken_at).num_seconds().max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}
