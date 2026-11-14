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
  guilab poll --timeout 600        wait for the human (a message, ...); prints what happened
  guilab say \"Done, have a look\" message the human
  guilab end                       end the session and kill its apps

Coordinates everywhere are screenshot pixels, at any scale. Sessions end after 30 idle
minutes. Apps see only their session (own Wayland display and XDG_RUNTIME_DIR). For
X11-only apps, start the session with `new -x` (Xwayland; apps get DISPLAY).

Locking: `input` needs the lock, so one agent drives a session at a time and the human
watching isn't fighting it. While a session is locked, run, kill, output changes, clip set
and end need the token too; read-only commands never do. Pass the token with -t TOKEN (on
every command: shells don't keep variables between calls) or $GUILAB_TOKEN.

Every command also prints, on stderr, what happened in the session since your previous
command (human input, windows, apps exiting), so you see what changed under you.
`guilab log` shows the full history.";

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
        /// Open the viewer window on this shell's Wayland display.
        #[arg(long)]
        show: bool,
    },
    /// Open the viewer window (on this shell's Wayland display) to watch and use sessions.
    Show,
    /// Close the viewer window. Sessions keep running.
    Hide,
    /// Start a session. NAME (or -S NAME) defaults to the git worktree's directory name.
    New {
        /// Session name.
        name: Option<String>,
        /// Output size in pixels.
        #[arg(short, long, value_name = "WxH", value_parser = parse_size)]
        size: Option<[u32; 2]>,
        /// Output scale (e.g. 2 or 1.5); screenshots stay in pixels.
        #[arg(long)]
        scale: Option<f64>,
        /// End after this many idle seconds (0 = never; default 1800).
        #[arg(long)]
        ttl: Option<u64>,
        /// Also run Xwayland, for X11-only apps (they get DISPLAY). Prefer Wayland otherwise.
        #[arg(short = 'x', long)]
        xwayland: bool,
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
        /// Where to save the PNG (default: the session dir, deleted when the session ends).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Capture now instead of waiting up to 3 s for apps to stop drawing.
        #[arg(long)]
        no_settle: bool,
    },
    /// Show or change the output size (pixels) and scale.
    Output {
        /// New size in pixels.
        #[arg(short, long, value_name = "WxH", value_parser = parse_size)]
        size: Option<[u32; 2]>,
        /// New scale (e.g. 2 or 1.5).
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
    /// Wait for the human: blocks until a wake event (by default: a human message, your
    /// lock revoked, an app exiting on its own, the session about to expire), prints it with
    /// everything else that happened since your last command, and exits.
    #[command(after_help = POLL_HELP)]
    Poll {
        /// Give up after SECS (exit code 2), still printing what happened meanwhile.
        #[arg(long, value_name = "SECS")]
        timeout: Option<f64>,
        /// Don't exit: print one line per wake event (for tools that watch output lines).
        #[arg(short, long)]
        follow: bool,
        /// Also wake on these event kinds or groups (comma-separated).
        #[arg(long, value_name = "KINDS")]
        wake: Vec<String>,
        /// Don't wake on these (they're reported with the next wake or command instead).
        #[arg(long, value_name = "KINDS")]
        passive: Vec<String>,
    },
    /// Show a message to the human (in the viewer's chat), optionally with images.
    Say {
        text: String,
        /// Attach an image (repeatable).
        #[arg(long, value_name = "PATH")]
        image: Vec<PathBuf>,
    },
    /// Show the session's event log (oldest first): commands, human input and messages,
    /// apps, windows, lock changes.
    Log {
        /// Only events since a time: 90s, 5m, 2h ago, or a clock time like 14:05 or 14:05:30.
        #[arg(long, value_name = "WHEN")]
        since: Option<String>,
        /// Only events after #SEQ.
        #[arg(long, value_name = "SEQ")]
        after: Option<u64>,
        /// Only these kinds or groups (comma-separated; see poll --help).
        #[arg(long, value_name = "KINDS")]
        kind: Vec<String>,
        /// Show at most the newest N events (0 = all).
        #[arg(short = 'n', long, default_value_t = 100)]
        last: usize,
    },
    /// Act as the human (for tests, until the viewer exists).
    #[command(hide = true)]
    Debug {
        #[command(subcommand)]
        op: DebugOp,
    },
}

const POLL_HELP: &str = "\
Event kinds: message revoke app_exit expiring (wake by default); motion button key scroll
touch gesture window_map window_unmap title output app_start lock unlock share (reported
with the next wake or command); command say screenshot (your own actions, only in `log`).
Groups: input (motion button key scroll touch gesture), window, app, all.
An app ended by `guilab kill` doesn't wake you.

Every guilab command also reports, on stderr, what happened since your previous command,
so you see what the human did. Clients without a token share one \"last seen\" position per
session; with -t TOKEN each lock holder has its own.";

#[derive(Subcommand)]
enum DebugOp {
    /// Send a message as the human.
    Message {
        text: String,
        #[arg(long, value_name = "PATH")]
        image: Vec<PathBuf>,
        /// A text annotation: "TEXT", "X,Y:TEXT" or "IMAGE@X,Y:TEXT" (IMAGE counts from 1).
        #[arg(long, value_name = "NOTE")]
        note: Vec<String>,
    },
    /// Send input as the human (same actions as `guilab input`).
    Input {
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        actions: Vec<String>,
    },
    /// Revoke the agent's lock.
    Revoke,
    /// Share the locked session with the human (on) or stop (off).
    Share { on: String },
}

#[derive(Subcommand)]
enum ClipOp {
    /// Print the clipboard's text.
    Get,
    /// Set the clipboard to TEXT (or stdin if no TEXT). Needs the token while locked.
    Set { text: Option<String> },
}

fn parse_note(s: &str) -> Result<guilab_proto::Annotation, String> {
    let mut a = guilab_proto::Annotation { text: s.to_string(), image: None, x: None, y: None, color: None };
    let Some((place, text)) = s.split_once(':') else { return Ok(a) };
    let (img, xy) = match place.split_once('@') {
        Some((i, xy)) => (Some(i), xy),
        None => (None, place),
    };
    let Some((x, y)) = xy.split_once(',') else { return Ok(a) };
    let (Ok(x), Ok(y)) = (x.trim().parse(), y.trim().parse()) else { return Ok(a) };
    if let Some(i) = img {
        let i: usize = i.parse().map_err(|_| format!("bad image number in note {s:?}"))?;
        a.image = Some(i.checked_sub(1).ok_or("images count from 1")?);
    }
    (a.text, a.x, a.y) = (text.to_string(), Some(x), Some(y));
    Ok(a)
}

/// `--since`: "90s"/"5m"/"2h" ago, or a local clock time today ("14:05", "14:05:30"). Unix ms.
fn parse_since(s: &str, tz: i64) -> Result<u64, String> {
    let now = guilab_proto::events::now_ms();
    let bad = || format!("bad time {s:?}: use 90s, 5m, 2h or 14:05[:30]");
    if s.contains(':') {
        let p: Vec<u64> = s.split(':').map(|x| x.parse().map_err(|_| bad())).collect::<Result<_, _>>()?;
        let secs = p[0] * 3600 + p.get(1).copied().unwrap_or(0) * 60 + p.get(2).copied().unwrap_or(0);
        let local_now = (now / 1000) as i64 + tz;
        let midnight = local_now - local_now.rem_euclid(86400) - tz;
        return Ok((midnight as u64 + secs) * 1000);
    }
    let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = n.parse().map_err(|_| bad())?;
    let mult = match unit {
        "s" | "" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return Err(bad()),
    };
    Ok(now.saturating_sub(n * mult * 1000))
}

/// Local UTC offset in seconds.
fn tz_offset() -> i64 {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() { 0 } else { tm.tm_gmtoff }
    }
}

fn abs(p: PathBuf) -> PathBuf {
    std::path::absolute(&p).unwrap_or(p)
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
    let fmt = guilab_proto::text::Fmt { tz: tz_offset() };
    let cmd = match cli.cmd {
        Cmd::Server { stop: false, idle_exit, show } => {
            let show = show.then(guilab_proto::own_wayland_socket).transpose()?;
            return client::run_server(idle_exit, show);
        }
        Cmd::Show => Command::Show { display: guilab_proto::own_wayland_socket()? },
        Cmd::Hide => Command::Hide,
        Cmd::Server { stop: true, .. } => return client::stop_server(),
        Cmd::New { name, size, scale, ttl, xwayland } => Command::New { name: name.or_else(|| cli.session.clone()), size, scale, ttl, xwayland },
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
        Cmd::Poll { timeout, follow, wake, passive } => {
            let tiers = guilab_proto::Tiers { wake, passive };
            tiers.check()?;
            Command::Poll { timeout_ms: timeout.map(|s| (s * 1000.0) as u64), follow, tiers }
        }
        Cmd::Say { text, image } => Command::Say { text, images: image.into_iter().map(abs).collect() },
        Cmd::Log { since, after, kind, last } => {
            guilab_proto::events::expand_kinds(&kind)?;
            Command::Log { after, since: since.map(|s| parse_since(&s, fmt.tz)).transpose()?, kinds: kind, last }
        }
        Cmd::Debug { op } => Command::Human {
            op: match op {
                DebugOp::Message { text, image, note } => guilab_proto::HumanOp::Message {
                    text,
                    images: image.into_iter().map(abs).collect(),
                    annotations: note.iter().map(|n| parse_note(n)).collect::<Result<_, _>>()?,
                },
                DebugOp::Input { actions } => guilab_proto::HumanOp::Input { actions: guilab_proto::input::parse(&actions)? },
                DebugOp::Revoke => guilab_proto::HumanOp::Revoke,
                DebugOp::Share { on } => guilab_proto::HumanOp::Share { on: matches!(on.as_str(), "on" | "1" | "true" | "yes") },
            },
        },
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
    let mut reply = c.request(target, cmd.clone())?;
    if let Command::Poll { follow, timeout_ms, .. } = &cmd {
        let timeout = timeout_ms.map(|ms| ms as f64 / 1000.0);
        loop {
            if !reply.ok {
                return Err(reply.error.unwrap_or_default());
            }
            let timed_out = format::poll(&reply, &fmt, cli.json, *follow, timeout)?;
            if timed_out {
                std::process::exit(2);
            }
            if !follow {
                return Ok(());
            }
            reply = c.next()?;
        }
    }
    // Events go to stderr, so stdout stays parseable (`$(guilab screenshot)`).
    if let Some(d) = &reply.events {
        if cli.json {
            eprintln!("{}", serde_json::to_string(d).unwrap_or_default());
        } else if let Some(b) = format::events_block(&fmt, d) {
            eprintln!("{b}");
        }
    }
    if !reply.ok {
        return Err(reply.error.unwrap_or_else(|| "unknown error".into()));
    }
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&reply.data).unwrap_or_default());
        return Ok(());
    }
    format::print(&cmd, reply.data, &fmt)
}
