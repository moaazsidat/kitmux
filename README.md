# kitmux

Save and restore [kitty](https://sw.kovidgoyal.net/kitty/) tabs, layouts, and the coding agent
sessions running in them.

Restart your machine, or have it crash, and get every tab back where it was, with each
Claude Code or omp conversation resumed in the tab it was in.

```console
$ kitmux save
saved 5 tabs and 4 agent sessions to ~/.local/state/kitmux/snapshots/2026-01-02T03-04-05Z.json

$ kitmux ls
  #  TAKEN               AGE  TABS  WINDOWS  AGENTS                    BY
  1  2026-01-02 03:04     2m     5        8  3 claude, 1 omp           timer
  2  2026-01-02 02:04    62m     5        8  3 claude, 1 omp           timer

$ kitmux resume          # after a restart: the last snapshot of the kitty that is gone
restored 5 tabs and resumed 4 agent sessions
```

## Why not kitty's own sessions?

kitty can save a session (`save_as_session --use-foreground-process`), but it records each
program's command line as it was typed. That doesn't bring back an agent conversation:
`claude` starts a new one, and `claude --resume` opens a picker. kitty also saves only when
you ask, so nothing has been saved when the machine crashes.

kitmux finds the session ID each agent is actually writing, saves on a timer, and resumes each
session in its own tab, with the same flags it was started with.

## Install

```bash
cargo install kitmux
kitmux timer install        # save every hour (systemd user timer)
```

kitmux talks to kitty over remote control. If you haven't already, add this to `kitty.conf` and
restart kitty:

```conf
allow_remote_control yes
listen_on unix:/tmp/kitty
```

kitty adds its pid to that path (`/tmp/kitty-1234`). kitmux finds it either way.

Optionally, bind keys to save right away, and to name the current tab. The second replaces
kitty's own tab rename (`set_tab_title`), whose names kitmux can't tell apart from titles
programs set:

```conf
map kitty_mod+s launch --type=background kitmux save
map kitty_mod+alt+t launch --type=overlay kitmux tab name
```

## Commands

| | |
|---|---|
| `kitmux save` | Snapshot every running kitty now. |
| `kitmux ls` | List snapshots, newest first. `--json` for scripts. |
| `kitmux show [snapshot]` | What a snapshot holds, and the command each agent resumes with. `--json` prints all of it. |
| `kitmux resume [snapshot]` | Restore tabs and layouts and resume agent sessions. `-n` shows the plan without running it. |
| `kitmux tab name [name]` | Name the current tab; the name is restored with it. Asks for a name if you don't give one. `--tab 2` names the second tab, `--clear` removes the name. |
| `kitmux tab ls` | List open tabs by position, with their names. |
| `kitmux timer install \| remove \| status` | Save on a schedule. `--every 30min`, `--keep 168`. |

A `snapshot` is a number from `kitmux ls`, `latest`, or a path to a snapshot file.

### What `resume` does

- **Adds tabs to the running kitty** and leaves the open ones alone. If kitty isn't running,
  kitmux starts it.
- **Restores each tab's layout**, including the full split tree, with each split's direction
  and size, for the `splits` layout. Focus goes back to the window and tab that had it.
- **Resumes agents by default.** Each agent runs under the window's shell, and you're back at
  that shell when it exits. `--no-agents` restores the layout with plain shells.
- **Never runs a session twice.** A tab whose agent session is already running is skipped.
  `--force` restores it anyway and opens a shell where the running agent would have gone.
- **Picks the right snapshot.** With no snapshot given, kitmux uses the newest one taken of a
  kitty that is no longer running. After a restart the timer may already have saved the new,
  nearly empty kitty, and that isn't the one you want back.
- **Doesn't rerun other programs.** Windows that weren't running an agent get a shell in the
  same directory. kitmux never reruns arbitrary commands.

### Agents

| Agent | How kitmux finds the session | Resumed with |
|---|---|---|
| [Claude Code](https://claude.com/claude-code) | `sessionId` in `<config dir>/sessions/<pid>.json` | `claude <flags> --resume <id>` |
| omp | the session `.jsonl` the process holds open | `omp <flags> --resume <id>` (`--continue-turns`: `--restore`) |

The session ID comes from the running process, not from its command line. A session picked
with `claude --resume` is restored correctly, and `CLAUDE_CONFIG_DIR` is carried over.

Flags the agent was started with are kept, such as `--model` and the permission mode. The
original prompt and resume flags are dropped, so the prompt isn't sent again and the right
session is picked.

Adding an agent means implementing `agent::Adapter` (see `src/agent/`).

## Snapshots

Snapshots are JSON files in `$KITMUX_STATE_DIR`, or else `$XDG_STATE_HOME/kitmux`, or else
`~/.local/state/kitmux`. The newest 168 are kept, a week of hourly saves (`--keep`, `KITMUX_KEEP`).

Each one holds what kitmux restores from (OS windows, tabs, layouts, windows, directories,
foreground processes, agent sessions) plus kitty's full `ls` output, unchanged. Command lines
and kitty's view of each window's environment can contain secrets, so the files are readable
only by you.

The timer skips a save when nothing has changed since the last snapshot.

## Limitations

- Linux only for now. Process details come from `/proc`, and the timer uses systemd.
- Remote control has to be unencrypted, so a `remote_control_password` isn't supported.
- A tab's title is restored only if you named it with `kitmux tab name`. Otherwise it's
  whatever the tab's program last set, which kitmux leaves alone.
- Scrollback isn't saved.

## License

MIT
