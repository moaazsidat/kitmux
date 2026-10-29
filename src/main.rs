//! kitmux: save and restore kitty tabs, layouts, and the coding agent sessions
//! running in them.

mod agent;
mod cmd;
mod kitty;
mod procfs;
mod restore;
mod snapshot;
mod store;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// kitty's remote control address, e.g. unix:/tmp/kitty-1234. By default
    /// every kitty that answers is used.
    #[arg(long, global = true, env = "KITMUX_KITTY_SOCKET")]
    socket: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Save the current tabs, layouts, and agent sessions.
    Save {
        /// For the timer: skip a snapshot identical to the last one, print
        /// nothing, and succeed when no kitty is running.
        #[arg(long)]
        timer: bool,
        /// How many snapshots to keep; older ones are deleted.
        #[arg(long, default_value_t = 168, env = "KITMUX_KEEP")]
        keep: usize,
    },
    /// List saved snapshots, newest first.
    Ls {
        #[arg(long)]
        json: bool,
    },
    /// Show what a snapshot holds and how each agent would be resumed.
    Show {
        /// A path, a number from `kitmux ls`, or `latest`.
        #[arg(default_value = "latest")]
        snapshot: String,
        /// Print the whole snapshot, including kitty's raw state.
        #[arg(long)]
        json: bool,
    },
    /// Restore a snapshot's tabs and layouts, resuming its agent sessions.
    ///
    /// Tabs are added to the running kitty, leaving the ones open now alone;
    /// with no kitty running, one is started. Tabs whose agent sessions are
    /// already running are skipped.
    Resume {
        /// A path, a number from `kitmux ls`, or `latest`. By default, the newest
        /// snapshot of a kitty that is no longer running.
        snapshot: Option<String>,
        /// Restore the layout only: every window gets a shell.
        #[arg(long)]
        no_agents: bool,
        /// Also continue turns that were cut off, where the agent supports
        /// it (omp --restore).
        #[arg(long)]
        continue_turns: bool,
        /// Restore tabs even if their agent sessions are running elsewhere;
        /// those windows get a shell rather than a second copy.
        #[arg(long)]
        force: bool,
        /// Show what would be restored without doing it.
        #[arg(long, short = 'n')]
        dry_run: bool,
    },
    /// Change tabs in ways kitmux remembers.
    Tab {
        #[command(subcommand)]
        action: TabAction,
    },
    /// Save automatically on a schedule (a systemd user timer).
    Timer {
        #[command(subcommand)]
        action: TimerAction,
    },
}

#[derive(Subcommand)]
enum TabAction {
    /// Name a tab. The name is its title from now on and is restored by
    /// `kitmux resume`.
    #[command(visible_alias = "set")]
    Name {
        /// The name. Without one, kitmux asks for it.
        #[arg(conflicts_with = "clear")]
        name: Option<String>,
        #[arg(long = "name", hide = true, conflicts_with_all = ["name", "clear"])]
        name_flag: Option<String>,
        /// Remove the tab's name.
        #[arg(long)]
        clear: bool,
        /// The tab: its position in its OS window (1 is leftmost), or `id:N` for
        /// its kitty id. By default, the tab kitmux runs in, else the active tab.
        #[arg(long, short)]
        tab: Option<String>,
    },
    /// List the open tabs and their names.
    Ls,
}

#[derive(Subcommand)]
enum TimerAction {
    /// Install and start the timer.
    Install {
        /// How often to save, as a systemd time span.
        #[arg(long, default_value = "1h")]
        every: String,
        /// How many snapshots to keep.
        #[arg(long, default_value_t = 168)]
        keep: usize,
    },
    /// Stop and remove the timer. Snapshots are kept.
    Remove,
    /// Show when it last ran and runs next.
    Status,
}

fn main() {
    let cli = Cli::parse();
    let socket = cli.socket.as_deref();
    let result = match cli.command {
        Command::Save { timer, keep } => cmd::save::run(socket, cmd::save::Options { timer, keep }),
        Command::Ls { json } => cmd::ls::list(json),
        Command::Show { snapshot, json } => cmd::ls::show(&snapshot, json),
        Command::Resume {
            snapshot,
            no_agents,
            continue_turns,
            force,
            dry_run,
        } => cmd::resume::run(
            socket,
            cmd::resume::Options {
                snapshot,
                restore: restore::Options {
                    agents: !no_agents,
                    resume: agent::ResumeOptions {
                        continue_turn: continue_turns,
                    },
                    force,
                },
                dry_run,
            },
        ),
        Command::Tab { action } => match action {
            TabAction::Name {
                name,
                name_flag,
                clear,
                tab,
            } => {
                let name = if clear { Some(String::new()) } else { name.or(name_flag) };
                cmd::tab::name(socket, tab.as_deref(), name)
            }
            TabAction::Ls => cmd::tab::ls(socket),
        },
        Command::Timer { action } => match action {
            TimerAction::Install { every, keep } => cmd::timer::install(&every, keep),
            TimerAction::Remove => cmd::timer::remove(),
            TimerAction::Status => cmd::timer::status(),
        },
    };
    if let Err(e) = result {
        eprintln!("kitmux: {e:#}");
        std::process::exit(1);
    }
}
