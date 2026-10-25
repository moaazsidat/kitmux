//! `kit save`: snapshot every running kitty.

use anyhow::Result;

use crate::kitty::discover;
use crate::snapshot::Snapshot;
use crate::store::Store;

pub struct Options {
    /// Run by the timer: say nothing when there is nothing to do, succeed when
    /// no kitty is running, and skip a snapshot identical to the last.
    pub timer: bool,
    /// How many snapshots to keep.
    pub keep: usize,
}

pub fn run(socket: Option<&str>, opts: Options) -> Result<()> {
    let clients = discover::find(socket)?;
    if clients.is_empty() {
        if opts.timer {
            return Ok(());
        }
        anyhow::bail!(
            "no kitty is reachable. kit needs remote control: add `allow_remote_control yes` \
             and `listen_on unix:/tmp/kitty` to kitty.conf and restart kitty, or pass --socket"
        );
    }
    let reason = if opts.timer { "timer" } else { "manual" };
    let snap = Snapshot::capture(&clients, reason)?;
    let store = Store::open_default()?;

    if opts.timer {
        if let Some(last) = store.list()?.first() {
            if last.snapshot.fingerprint() == snap.fingerprint() {
                return Ok(());
            }
        }
    }

    let path = store.save(&snap)?;
    store.prune(opts.keep.max(1))?;
    if !opts.timer {
        let tabs = snap.tabs().count();
        let agents = snap.agents().count();
        println!("saved {tabs} tabs and {agents} agent sessions to {}", path.display());
    }
    Ok(())
}
