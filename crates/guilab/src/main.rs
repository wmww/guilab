//! `guilab` CLI. Every command except `server` talks to the server over the control
//! socket, starting it first if needed.

mod build_id;
mod client;
mod format;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use guilab_proto::{Command, GlobalCmd, SessionCmd};

const AFTER_HELP: &str = "\
Quick start:
  guilab new                       start a session (named after the git worktree; -s 1280x720)
  guilab run -- gtk4-demo          start an app in it (its log: guilab ps)
  guilab screenshot                capture an image (waits for apps to finish drawing), prints its PNG path
  guilab tree                      windows and popups: ids, pixel geometry, app ids, titles
  guilab work \"testing the menu\"   tell the human what you're doing (needed for lock/input)
  guilab lock                      take the session's lock; prints a TOKEN
  guilab -t TOKEN input move 100 200 click     send input (guilab input --help)
  guilab done \"Fixed, have a look\" message the human, release the lock, wait for them
  guilab poll --timeout 300        wait for the human's reply; prints it
  guilab end                       end the session and kill its apps

Coordinates everywhere are screenshot pixels, at any scale. Sessions end after 30 idle
minutes. Apps see only their session (own Wayland display and XDG_RUNTIME_DIR). For
X11-only apps, start the session with `new -x` (Xwayland; apps get DISPLAY).

Locking: `input` needs the lock, so one agent drives a session at a time and the human
watching isn't fighting it. While a session is locked, run, kill, output changes, clip set
and end need the token too; read-only commands never do. Pass the token with -t TOKEN (on
every command: shells don't keep variables between calls) or $GUILAB_TOKEN.

Every command also prints, on stderr, what happened in the session since your previous
command (human input, windows, apps exiting), so you see what changed under you, and
says when a human message is waiting. Messages are only shown by `guilab poll`.
`guilab log` shows the full history.

Testing guilab itself: run the inner server as an app with its own directory
(`guilab run -- env GUILAB_DIR=/run/user/$UID/gl-inner guilab server --show`), then drive it
with `GUILAB_DIR=/run/user/$UID/gl-inner GUILAB_NO_AUTOSTART=1 guilab ...`.";

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
        /// Don't run a server: wait (up to 30 s) until one answers, e.g. one being restarted.
        #[arg(long, conflicts_with_all = ["stop", "show"])]
        wait: bool,
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
    /// Capture the output as a new image and save it as PNG; prints the path and the image
    /// id. Waits for apps to finish drawing unless --no-settle.
    Screenshot {
        /// Where to save the PNG (default: the session dir, deleted when the session ends).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Capture now instead of waiting up to 3 s for apps to stop drawing.
        #[arg(long)]
        no_settle: bool,
        /// Draw the mouse cursor in (the app's own cursor, at the pointer).
        #[arg(long)]
        cursor: bool,
        /// Crop the image to this part of the screen ('X,Y WxH'; the image keeps the rest).
        #[arg(long, value_name = "X,Y WxH")]
        crop: Option<String>,
        /// Shrink the PNG to fit WxH.
        #[arg(long, value_name = "WxH", value_parser = parse_size)]
        max: Option<[u32; 2]>,
    },
    /// Render an image to PNG (prints the path), or list its elements; `image make` makes one.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true, after_help = IMAGE_HELP)]
    Image {
        #[command(subcommand)]
        make: Option<ImageCmd>,
        /// Image id, e.g. k3f9.
        #[arg(required = true)]
        id: Option<String>,
        /// Where to save the PNG (default: the session dir).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Render this part of the canvas ('X,Y WxH') instead of the image's crop; `none` = all.
        #[arg(long, value_name = "X,Y WxH|none")]
        crop: Option<String>,
        /// Shrink the PNG to fit WxH.
        #[arg(long, value_name = "WxH", value_parser = parse_size)]
        max: Option<[u32; 2]>,
        /// Pictures only, without marks.
        #[arg(long)]
        no_marks: bool,
        /// Print the element list instead of rendering.
        #[arg(long)]
        list: bool,
        /// With --list: print pen points instead of their bounding rectangles.
        #[arg(long)]
        points: bool,
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
    /// Take the session's lock (needed for input; run `work` first). Prints a token to pass with -t.
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
    /// everything else that happened since your last command, and exits. Returns at once if
    /// a human message is waiting. Blocking needs `guilab done` first.
    #[command(after_help = poll_help())]
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
    /// Tell the human what you're working on (shown live in the viewer's chat). Needed
    /// before `lock` and `input`; run it again to update. End with `guilab done`.
    Work {
        /// What you're doing now, e.g. "rebuilding after the layout fix".
        status: String,
        /// Also post this message to the human's chat.
        #[arg(short, long, value_name = "MSG")]
        message: Option<String>,
        /// Attach an image (an image id or a PNG/JPEG/SVG file; repeatable).
        #[arg(long, value_name = "ID|PATH")]
        image: Vec<String>,
    },
    /// You're finished: post MSG (what you did, or what you need) to the human's chat,
    /// release the lock, and show them you're waiting. Then wait with `guilab poll`.
    Done {
        /// Message for the human.
        message: Option<String>,
        /// Attach an image (an image id or a PNG/JPEG/SVG file; repeatable).
        #[arg(long, value_name = "ID|PATH")]
        image: Vec<String>,
    },
    /// Replaced by `work -m` and `done`.
    #[command(hide = true)]
    Say {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
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
    /// Act as the human, like the viewer does (the tests' stand-in for it).
    #[command(hide = true)]
    Debug {
        #[command(subcommand)]
        op: DebugOp,
    },
}

/// `poll --help` after the options; kinds, tiers and groups come from the event table.
fn poll_help() -> String {
    use guilab_proto::{Tier, events::{GROUPS, KINDS}};
    let of = |t: Tier| KINDS.iter().filter(|(_, x)| *x == t).map(|(k, _)| *k).collect::<Vec<_>>().join(" ");
    let groups: Vec<&str> = GROUPS.iter().map(|(g, _)| *g).filter(|g| *g != "input").collect();
    let input = GROUPS.iter().find(|(g, _)| *g == "input").map(|(_, ks)| ks.join(" ")).unwrap_or_default();
    let kinds = format!(
        "Event kinds: {} (wake by default); {} (reported with the next wake or command); {} (your own actions, only in `log`).",
        of(Tier::Wake),
        of(Tier::Passive),
        of(Tier::Log)
    );
    format!(
        "{}\nGroups: input ({input}), {}, all.\nAn app ended by `guilab kill` doesn't wake you.\n\n{}",
        wrap(&kinds, 90),
        groups.join(", "),
        "Human messages stay unread until a poll prints them; other commands only say one is
waiting. A poll that hands you a message marks you working (\"read your message\"), so end
with `guilab done`. Every guilab command also reports, on stderr, what else happened since
your previous command. Each git worktree has its own \"last seen\" position per session."
    )
}

/// Wraps text at `width` columns, on spaces.
fn wrap(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut line = 0;
    for w in text.split(' ') {
        if line > 0 && line + 1 + w.len() > width {
            out.push('\n');
            line = 0;
        } else if line > 0 {
            out.push(' ');
            line += 1;
        }
        out.push_str(w);
        line += w.len();
    }
    out
}

#[derive(Subcommand)]
enum DebugOp {
    /// Send a message as the human (draw with `guilab image make`, attach by id).
    Message {
        text: String,
        #[arg(long, value_name = "ID|PATH")]
        image: Vec<String>,
        /// Draw this element on a screenshot taken now (repeatable; like the viewer's tools).
        #[arg(long, value_name = "ELEMENT")]
        draw: Vec<String>,
    },
    /// Send input as the human (same actions as `guilab input`).
    Input {
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        actions: Vec<String>,
    },
    /// Revoke the agent's lock.
    Revoke,
    /// Insert a picture file in the viewer (as File > Insert image does).
    Insert { path: PathBuf },
}

const IMAGE_HELP: &str = "\
Coordinates are canvas pixels (= screenshot pixels), even in cropped or shrunk renders.
Element syntax, also for `image make`: see the guilab skill doc (Images).";

#[derive(Subcommand)]
enum ImageCmd {
    /// Make a new image: from a copy of --from, or from scratch. Prints its listing and a
    /// preview PNG. Send it with: guilab done "..." --image ID
    Make {
        /// Start from a copy of this image (canvas, scale, crop, elements).
        #[arg(long, value_name = "ID")]
        from: Option<String>,
        /// Canvas size (default with --from: its canvas; else just big enough).
        #[arg(long, value_name = "WxH", value_parser = parse_size)]
        canvas: Option<[u32; 2]>,
        /// Background color (default: transparent), or none.
        #[arg(long, value_name = "COLOR")]
        color: Option<String>,
        /// The new image's crop ('X,Y WxH'), or none.
        #[arg(long, value_name = "X,Y WxH|none")]
        crop: Option<String>,
        /// Remove these elements of --from (numbers from its listing).
        #[arg(long, value_name = "N,N", value_delimiter = ',')]
        drop: Vec<u32>,
        /// Element lines, one per argument; a number first replaces that element of --from;
        /// `-` reads more lines from stdin.
        #[arg(value_name = "ELEMENT")]
        elements: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ClipOp {
    /// Print the clipboard's text.
    Get,
    /// Set the clipboard to TEXT (or stdin if no TEXT). Needs the token while locked.
    Set { text: Option<String> },
}

/// `--image ID|PATH`: a file path (anything with a `/` or `.`, or an existing file) is sent
/// absolute; anything else is an image id.
fn image_arg(s: &str) -> String {
    let p = std::path::Path::new(s);
    if s.contains(['/', '.']) || p.exists() { abs(p.to_path_buf()).display().to_string() } else { s.to_string() }
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
    let fmt = guilab_proto::text::Fmt::local();
    let cmd: Command = match cli.cmd {
        Cmd::Server { wait: true, .. } => return client::wait_server(),
        Cmd::Server { stop: false, idle_exit, show, .. } => {
            let show = show.then(guilab_proto::own_wayland_socket).transpose()?;
            return client::run_server(idle_exit, show, build_id::build_id());
        }
        Cmd::Show => GlobalCmd::Show { display: guilab_proto::own_wayland_socket()? }.into(),
        Cmd::Hide => GlobalCmd::Hide.into(),
        Cmd::Server { stop: true, .. } => return client::stop_server(),
        Cmd::New { name, size, scale, ttl, xwayland } => GlobalCmd::New { name: name.or_else(|| cli.session.clone()), size, scale, ttl, xwayland }.into(),
        Cmd::End => SessionCmd::End.into(),
        Cmd::Ls => GlobalCmd::Ls.into(),
        Cmd::Run { cmd } => SessionCmd::Run {
            argv: cmd,
            cwd: std::env::current_dir().map_err(|e| e.to_string())?,
            env: std::env::vars().collect(),
        }.into(),
        Cmd::Kill { id } => SessionCmd::Kill { id }.into(),
        Cmd::Ps => SessionCmd::Ps.into(),
        Cmd::Tree => SessionCmd::Tree.into(),
        Cmd::Screenshot { output, no_settle, cursor, crop, max } => {
            SessionCmd::Screenshot { path: output.map(abs), settle: !no_settle, cursor, crop, max }.into()
        }
        Cmd::Image { make: Some(ImageCmd::Make { from, canvas, color, crop, drop, elements }), .. } => {
            let mut stdin = None;
            let mut lines = Vec::new();
            for e in elements {
                if e == "-" {
                    if stdin.is_none() {
                        stdin = Some(std::io::read_to_string(std::io::stdin()).map_err(|e| format!("reading stdin: {e}"))?);
                    }
                    lines.push(stdin.clone().unwrap_or_default());
                } else {
                    lines.push(e);
                }
            }
            SessionCmd::ImageMake { from, canvas, color, crop, drop, elements: lines, cwd: std::env::current_dir().map_err(|e| e.to_string())? }.into()
        }
        Cmd::Image { make: None, id, output, crop, max, no_marks, list, points } => {
            SessionCmd::Image { id: id.unwrap_or_default(), path: output.map(abs), crop, max, marks: !no_marks, list, points }.into()
        }
        Cmd::Output { size, scale } => SessionCmd::Output { size, scale }.into(),
        Cmd::Lock { force } => SessionCmd::Lock { force }.into(),
        Cmd::Unlock { force } => SessionCmd::Unlock { force }.into(),
        Cmd::Input { actions } => SessionCmd::Input { actions: guilab_proto::input::parse(&actions)? }.into(),
        Cmd::Poll { timeout, follow, wake, passive } => {
            let tiers = guilab_proto::Tiers { wake, passive };
            tiers.resolve()?;
            SessionCmd::Poll { timeout_ms: timeout.map(|s| (s * 1000.0) as u64), follow, tiers }.into()
        }
        Cmd::Work { status, message, image } => SessionCmd::Work { status, message, images: image.iter().map(|i| image_arg(i)).collect() }.into(),
        Cmd::Done { message, image } => SessionCmd::Done { message, images: image.iter().map(|i| image_arg(i)).collect() }.into(),
        Cmd::Say { .. } => {
            return Err("`guilab say` is gone. While you keep working: guilab work \"what you're doing\" -m \"MESSAGE\". \
                        When you're finished: guilab done \"MESSAGE\" (then wait with guilab poll)"
                .into());
        }
        Cmd::Log { since, after, kind, last } => {
            guilab_proto::events::expand_kinds(&kind)?;
            SessionCmd::Log { after, since: since.map(|s| parse_since(&s, fmt.tz)).transpose()?, kinds: kind, last }.into()
        }
        Cmd::Debug { op } => SessionCmd::Human {
            op: match op {
                DebugOp::Message { text, image, draw } => {
                    guilab_proto::HumanOp::Message { text, images: image.iter().map(|i| image_arg(i)).collect(), draw }
                }
                DebugOp::Input { actions } => guilab_proto::HumanOp::Input { actions: guilab_proto::input::parse(&actions)? },
                DebugOp::Revoke => guilab_proto::HumanOp::Revoke,
                DebugOp::Insert { path } => guilab_proto::HumanOp::Insert { path: abs(path).display().to_string() },
            },
        }.into(),
        Cmd::Clip { op: ClipOp::Get } => SessionCmd::ClipGet.into(),
        Cmd::Clip { op: ClipOp::Set { text } } => SessionCmd::ClipSet {
            text: match text {
                Some(t) => t,
                None => std::io::read_to_string(std::io::stdin()).map_err(|e| format!("reading stdin: {e}"))?,
            },
        }.into(),
    };
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    let target = guilab_proto::Target {
        name: cli.session.or_else(|| env("GUILAB_SESSION")),
        root: None,
        token: cli.token.or_else(|| env("GUILAB_TOKEN")),
    };
    let mut c = client::connect()?;
    let mut reply = c.request(target.clone(), cmd.clone())?;
    if let Command::Session(SessionCmd::Poll { follow, timeout_ms, .. }) = &cmd {
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
        print_events(d, &fmt, cli.json);
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

/// Events attached to a reply, on stderr.
fn print_events(d: &guilab_proto::Delivery, fmt: &guilab_proto::text::Fmt, json: bool) {
    if json {
        eprintln!("{}", serde_json::to_string(d).unwrap_or_default());
    } else if let Some(b) = format::events_block(fmt, d) {
        eprintln!("{b}");
    }
}
