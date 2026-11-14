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
    if lines.is_empty() {
        return None;
    }
    Some(format!("[guilab events {}; not errors]\n{}", fmt.since(d), lines.join("\n")))
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
            if !s.settled {
                line += " (not settled: still drawing or animating)";
            }
            line
        }
        Command::Output { .. } => output(&get(v)?),
        Command::Lock { .. } => {
            let l: LockInfo = get(v)?;
            format!(
                "token {} (session {}). Pass it to commands that change the session: guilab -t {} input ...",
                l.token, l.session, l.token
            )
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
        Command::Say { .. } => "sent to the viewer's chat".into(),
        Command::ClipSet { .. } | Command::Human { .. } | Command::Poll { .. } => return Ok(()),
        Command::Log { .. } => {
            let l: LogInfo = get(v)?;
            let mut out: Vec<String> = Vec::new();
            if l.dropped > 0 {
                out.push(format!("({} oldest events were dropped)", l.dropped));
            }
            if l.omitted > 0 {
                out.push(format!("({} earlier events not shown; -n 0 shows all)", l.omitted));
            }
            out.extend(l.events.iter().map(|e| fmt.log_line(e)));
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
