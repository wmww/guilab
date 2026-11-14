//! `guilab` CLI. Every command except `server` talks to the server over the control
//! socket, starting it first if needed.

mod client;
mod format;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use guilab_proto::Command;

const AFTER_HELP: &str = "\
Quick start:
  guilab new                       start a session (named after the git worktree; -s 1280x720)
  guilab run -- gtk4-demo          start an app in it (its log: guilab ps)
  guilab screenshot                save a PNG (waits for apps to finish drawing), prints its path
  guilab tree                      windows and popups: ids, pixel geometry, app ids, titles
  guilab lock                      take the session's lock; prints a TOKEN
  guilab -t TOKEN input move 100 200 click     send input (guilab input --help)
  guilab -t TOKEN unlock
  guilab end                       end the session and kill its apps

Coordinates everywhere are screenshot pixels, at any scale. Sessions end after 30 idle
minutes. Apps see only their session (own Wayland display and XDG_RUNTIME_DIR).

Locking: `input` needs the lock, so one agent drives a session at a time and the human
watching isn't fighting it. While a session is locked, run, kill, output changes, clip set
and end need the token too; read-only commands never do. Pass the token with -t TOKEN (on
every command: shells don't keep variables between calls) or $GUILAB_TOKEN.";

#[derive(Parser)]
#[command(name = "guilab", version, about = "Headless GUI sessions for humans and agents", disable_help_subcommand = true, after_help = AFTER_HELP)]
struct Cli {
    /// Session to act on (default: $GUILAB_SESSION, else this worktree's, else the only one).
    #[arg(long, short = 'S', global = true)]
    session: Option<String>,
    /// Lock token from `guilab lock` (default: $GUILAB_TOKEN).
    #[arg(long, short = 't', global = true, value_name = "TOKEN")]
    token: Option<String>,
    /// Print structured JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the server in the foreground (normally autostarted).
    Server {
        /// Stop the running server instead (ends all sessions).
        #[arg(long)]
        stop: bool,
        /// Exit after this many seconds without sessions (0 = never).
        #[arg(long, default_value_t = 0)]
        idle_exit: u64,
    },
    /// Start a session. NAME defaults to the git worktree's directory name.
    New {
        name: Option<String>,
        /// Output size in pixels.
        #[arg(short, long, value_name = "WxH", value_parser = parse_size)]
        size: Option<[u32; 2]>,
        #[arg(long)]
        scale: Option<f64>,
        /// End after this many idle seconds (0 = never; default 1800).
        #[arg(long)]
        ttl: Option<u64>,
    },
    /// End the session, killing everything in it.
    End,
    /// List sessions.
    Ls,
    /// Start an app in the session: guilab run -- CMD [ARGS...]
    Run {
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },
    /// Kill an app by id, or all apps.
    Kill { id: Option<u32> },
    /// List apps with their state and log file.
    Ps,
    /// Show windows, popups and subsurfaces (ids, app_id, title, pixel geometry).
    Tree,
    /// Save a PNG of the output. Waits for apps to finish drawing unless --no-settle.
    Screenshot {
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        no_settle: bool,
    },
    /// Show or change the output size (pixels) and scale.
    Output {
        #[arg(short, long, value_name = "WxH", value_parser = parse_size)]
        size: Option<[u32; 2]>,
        #[arg(long)]
        scale: Option<f64>,
    },
    /// Take the session's lock (needed for input). Prints a token to pass with -t.
    Lock {
        /// Take it from another holder (only if they're gone).
        #[arg(long)]
        force: bool,
    },
    /// Release the lock and anything still held (keys, buttons, touches).
    Unlock {
        /// Release someone else's lock.
        #[arg(long)]
        force: bool,
    },
    /// Send input: guilab -t TOKEN input move X Y click type TEXT key Return ...
    #[command(long_about = guilab_proto::input::HELP)]
    Input {
        /// Actions, run in order (see above).
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true, value_name = "ACTION")]
        actions: Vec<String>,
    },
    /// Read or set the session's clipboard (text).
    Clip {
        #[command(subcommand)]
        op: ClipOp,
    },
}

#[derive(Subcommand)]
enum ClipOp {
    /// Print the clipboard's text.
    Get,
    /// Set the clipboard to TEXT (or stdin if no TEXT). Needs the token while locked.
    Set { text: Option<String> },
}

fn parse_size(s: &str) -> Result<[u32; 2], String> {
    let (w, h) = s.split_once(['x', 'X']).ok_or("expected WxH, e.g. 1280x720")?;
    Ok([w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?])
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = real_main(cli) {
        eprintln!("guilab: {e}");
        std::process::exit(1);
    }
}

fn real_main(cli: Cli) -> Result<(), String> {
    let cmd = match cli.cmd {
        Cmd::Server { stop: false, idle_exit } => return client::run_server(idle_exit),
        Cmd::Server { stop: true, .. } => return client::stop_server(),
        Cmd::New { name, size, scale, ttl } => Command::New { name, size, scale, ttl },
        Cmd::End => Command::End,
        Cmd::Ls => Command::Ls,
        Cmd::Run { cmd } => Command::Run {
            argv: cmd,
            cwd: std::env::current_dir().map_err(|e| e.to_string())?,
            env: std::env::vars().collect(),
        },
        Cmd::Kill { id } => Command::Kill { id },
        Cmd::Ps => Command::Ps,
        Cmd::Tree => Command::Tree,
        Cmd::Screenshot { output, no_settle } => Command::Screenshot {
            path: output.map(|p| std::path::absolute(&p).unwrap_or(p)),
            settle: !no_settle,
        },
        Cmd::Output { size, scale } => Command::Output { size, scale },
        Cmd::Lock { force } => Command::Lock { force },
        Cmd::Unlock { force } => Command::Unlock { force },
        Cmd::Input { actions } => Command::Input { actions: guilab_proto::input::parse(&actions)? },
        Cmd::Clip { op: ClipOp::Get } => Command::ClipGet,
        Cmd::Clip { op: ClipOp::Set { text } } => Command::ClipSet {
            text: match text {
                Some(t) => t,
                None => std::io::read_to_string(std::io::stdin()).map_err(|e| format!("reading stdin: {e}"))?,
            },
        },
    };
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    let target = guilab_proto::Target {
        name: cli.session.or_else(|| env("GUILAB_SESSION")),
        root: None,
        token: cli.token.or_else(|| env("GUILAB_TOKEN")),
    };
    let mut c = client::connect()?;
    let reply = c.request(target, cmd.clone())?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&reply).unwrap_or_default());
        return Ok(());
    }
    format::print(&cmd, reply)
}
