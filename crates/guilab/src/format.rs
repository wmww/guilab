//! Compact text output for agents.

use guilab_proto::{
    Delivery, Response,
    events::{LogInfo, PollInfo},
    text::Fmt,
};
use guilab_proto::{Command, InputInfo, LockInfo, NodeKind, OutputInfo, ProcInfo, ProcState, SessionInfo, ShotInfo, TreeNode};
use serde::de::DeserializeOwned;

fn get<T: DeserializeOwned>(v: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(v).map_err(|e| format!("bad reply: {e}"))
}

fn output(o: &OutputInfo) -> String {
    if o.scale == 1.0 { format!("{}x{}", o.width, o.height) } else { format!("{}x{} scale {}", o.width, o.height, o.scale) }
}

fn session(s: &SessionInfo) -> String {
    let mut line = format!("{} {} apps={} windows={}", s.name, output(&s.output), s.apps, s.windows);
    if let Some(d) = &s.x_display {
        line += &format!(" DISPLAY={d}");
    }
    if let Some(e) = s.expires_in {
        line += &format!(" expires={}m", e.div_ceil(60));
    }
    if let Some(r) = &s.root {
        line += &format!(" root={}", r.display());
    }
    if s.in_viewer {
        line += " (shown in the viewer)";
    }
    if let Some(w) = &s.working {
        line += &format!(" working={w:?}");
    }
    line
}

fn proc_line(p: &ProcInfo) -> String {
    let state = match &p.state {
        ProcState::Running => "running".to_string(),
        ProcState::Exited { code } => format!("exited {code}"),
        ProcState::Signaled { signal } => format!("killed by signal {signal}"),
    };
    format!("{} pid={} {} {} log={}", p.id, p.pid, state, p.argv.join(" "), p.log.display())
}

fn tree(nodes: &[TreeNode], depth: usize, out: &mut String) {
    for n in nodes {
        let kind = match n.kind {
            NodeKind::Toplevel => "toplevel",
            NodeKind::Popup => "popup",
            NodeKind::Subsurface => "subsurface",
            NodeKind::X11 => "x11",
            NodeKind::X11Popup => "x11_popup",
        };
        out.push_str(&"  ".repeat(depth));
        out.push_str(&format!("{} {kind} {},{} {}x{}", n.id, n.x, n.y, n.width, n.height));
        if let Some(x) = n.xid {
            out.push_str(&format!(" xid={x:#x}"));
        }
        if let Some(c) = &n.class {
            out.push_str(&format!(" class={c}"));
        }
        if let Some(a) = &n.app_id {
            out.push_str(&format!(" app_id={a}"));
        }
        if let Some(t) = &n.title {
            out.push_str(&format!(" title={t:?}"));
        }
        if let Some(p) = n.pid {
            out.push_str(&format!(" pid={p}"));
        }
        out.push('\n');
        tree(&n.children, depth + 1, out);
    }
}

/// Events attached to a command reply, as printed on stderr.
pub fn events_block(fmt: &Fmt, d: &Delivery) -> Option<String> {
    let lines = fmt.delivery(d);
    let mut out = Vec::new();
    if !lines.is_empty() {
        out.push(format!("[guilab events {}; not errors]\n{}", fmt.since(d), lines.join("\n")));
    }
    if let Some(u) = &d.unread {
        out.push(fmt.unread(u));
    }
    (!out.is_empty()).then(|| out.join("\n"))
}

/// Prints one poll reply. Returns whether it timed out.
pub fn poll(r: &Response, fmt: &Fmt, json: bool, follow: bool, timeout: Option<f64>) -> Result<bool, String> {
    let info: PollInfo = get(r.data.clone())?;
    let empty = Delivery::default();
    let d = r.events.as_ref().unwrap_or(&empty);
    if json {
        let v = serde_json::json!({ "timed_out": info.timed_out, "since": d.since, "events": d.events });
        println!("{}", serde_json::to_string(&v).unwrap_or_default());
    } else if follow {
        for l in fmt.follow_lines(d) {
            println!("{l}");
        }
        if info.timed_out {
            println!("timed out after {}s", timeout.unwrap_or(0.0));
        }
    } else {
        if info.timed_out {
            println!("Timed out: no wake event within {}s (exit code 2).", timeout.unwrap_or(0.0));
        }
        let lines = fmt.delivery(d);
        // Only the wake event: no heading needed.
        let only_wake = !info.timed_out && d.events.len() == 1;
        if !lines.is_empty() && !only_wake {
            let s = fmt.since(d);
            println!("{}{}:", s[..1].to_uppercase(), &s[1..]);
        }
        for l in lines {
            println!("{l}");
        }
    }
    use std::io::Write;
    let _ = std::io::stdout().flush();
    Ok(info.timed_out)
}

pub fn print(cmd: &Command, v: serde_json::Value, fmt: &Fmt) -> Result<(), String> {
    let text = match cmd {
        Command::New { .. } => {
            let s: SessionInfo = get(v)?;
            format!("{}\nWAYLAND_DISPLAY={}", session(&s), s.wayland_display.display())
        }
        Command::End => format!("ended {}", get::<SessionInfo>(v)?.name),
        Command::Ls => {
            let l: Vec<SessionInfo> = get(v)?;
            if l.is_empty() { "no sessions".into() } else { l.iter().map(session).collect::<Vec<_>>().join("\n") }
        }
        Command::Run { .. } => proc_line(&get(v)?),
        Command::Kill { .. } => {
            let l: Vec<ProcInfo> = get(v)?;
            if l.is_empty() { "nothing running".into() } else { l.iter().map(|p| format!("killed {} pid={}", p.id, p.pid)).collect::<Vec<_>>().join("\n") }
        }
        Command::Ps => {
            let l: Vec<ProcInfo> = get(v)?;
            if l.is_empty() { "no apps".into() } else { l.iter().map(proc_line).collect::<Vec<_>>().join("\n") }
        }
        Command::Tree => {
            let l: Vec<TreeNode> = get(v)?;
            let mut s = String::new();
            tree(&l, 0, &mut s);
            if s.is_empty() { "no windows".into() } else { s.trim_end().to_string() }
        }
        Command::Screenshot { .. } => {
            let s: ShotInfo = get(v)?;
            let mut line = format!("{} {}x{}", s.path.display(), s.width, s.height);
            if s.animating {
                line += " (the app keeps animating; taken once input had time to land)";
            } else if !s.settled {
                line += " (not settled: still drawing when the wait ran out)";
            }
            for c in &s.cropped {
                line += &format!(
                    "\nwindow {} is {}x{} and doesn't fit the {}x{} output, so it's cut off (bigger output: guilab output -s WxH)",
                    c.id, c.width, c.height, s.width, s.height
                );
            }
            line
        }
        Command::Output { .. } => output(&get(v)?),
        Command::Lock { .. } => {
            let l: LockInfo = get(v)?;
            // Only the token on stdout, so `T=$(guilab lock)` works; the hint goes to stderr.
            eprintln!("locked session {}. Pass the token to commands that change it: guilab -t {} input ...", l.session, l.token);
            l.token
        }
        Command::Unlock { .. } => "unlocked".into(),
        Command::Input { .. } => {
            let i: InputInfo = get(v)?;
            let n = |v: f64| if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v}") };
            let mut line = format!("pointer {},{}", n(i.pointer[0]), n(i.pointer[1]));
            match i.under {
                Some(s) => line += &format!(" over surface {s}"),
                None => line += " over nothing",
            }
            match i.focus {
                Some(s) => line += &format!("; keyboard focus {s}"),
                None => line += "; no keyboard focus",
            }
            if let Some(s) = i.pressed_on {
                line += &format!("; the last press was on surface {s} (gone or moved since)");
            }
            line
        }
        Command::ClipGet => {
            let t: String = get(v)?;
            print!("{t}");
            if !t.ends_with('\n') {
                println!();
            }
            return Ok(());
        }
        Command::Work { status, message, images } => {
            let posted = if message.is_some() || !images.is_empty() { "; message posted" } else { "" };
            format!("working: {status}{posted}")
        }
        Command::Done { message, images } => {
            let info: guilab_proto::DoneInfo = get(v)?;
            let mut lines = Vec::new();
            if message.is_some() || !images.is_empty() {
                lines.push("message posted".to_string());
            }
            if info.unlocked {
                lines.push("released your lock".into());
            }
            lines.push("marked done: the human sees you're waiting for them. Wait for their reply with: guilab poll --timeout 600".into());
            lines.join("\n")
        }
        Command::ClipSet { .. } | Command::Human { .. } | Command::Poll { .. } => return Ok(()),
        Command::Log { .. } => {
            let l: LogInfo = get(v)?;
            let mut out: Vec<String> = Vec::new();
            if l.dropped > 0 {
                out.push(format!("({} oldest events were dropped)", l.dropped));
            }
            let omitted = format!("({} earlier events not shown; -n 0 shows all)", l.omitted);
            if l.omitted > 0 {
                out.push(omitted.clone());
            }
            out.extend(l.events.iter().map(|e| fmt.log_line(e)));
            // Also at the end, where `| tail` sees it.
            if l.omitted > 0 && l.events.len() > 20 {
                out.push(omitted);
            }
            if out.is_empty() { "no events".into() } else { out.join("\n") }
        }
        Command::Show { .. } => {
            let i: guilab_proto::ViewerInfo = get(v)?;
            let how = if i.buffers == "shm" { ", shm: CPU copy" } else { "" };
            format!("viewer shown on {}{how}", i.display.display())
        }
        Command::Ping | Command::Shutdown | Command::Hide => return Ok(()),
    };
    println!("{text}");
    Ok(())
}
