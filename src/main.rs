//! kit: save and restore kitty tabs, layouts, and the coding agent sessions
//! running in them.

mod agent;
mod cmd;
mod kitty;
mod procfs;
mod snapshot;
mod store;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// kitty's remote control address, e.g. unix:/tmp/kitty-1234. By default
    /// every kitty that answers is used.
    #[arg(long, global = true, env = "KIT_KITTY_SOCKET")]
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
        #[arg(long, default_value_t = 168, env = "KIT_KEEP")]
        keep: usize,
    },
    /// List saved snapshots, newest first.
    Ls {
        #[arg(long)]
        json: bool,
    },
    /// Show what a snapshot holds and how each agent would be resumed.
    Show {
        /// A path, a number from `kit ls`, or `latest`.
        #[arg(default_value = "latest")]
        snapshot: String,
        /// Print the whole snapshot, including kitty's raw state.
        #[arg(long)]
        json: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let socket = cli.socket.as_deref();
    let result = match cli.command {
        Command::Save { timer, keep } => cmd::save::run(socket, cmd::save::Options { timer, keep }),
        Command::Ls { json } => cmd::ls::list(json),
        Command::Show { snapshot, json } => cmd::ls::show(&snapshot, json),
    };
    if let Err(e) = result {
        eprintln!("kit: {e:#}");
        std::process::exit(1);
    }
}
