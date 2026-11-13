//! Compact text output for agents.

use guilab_proto::{Command, NodeKind, OutputInfo, ProcInfo, ProcState, SessionInfo, ShotInfo, TreeNode};
use serde::de::DeserializeOwned;

fn get<T: DeserializeOwned>(v: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(v).map_err(|e| format!("bad reply: {e}"))
}

fn output(o: &OutputInfo) -> String {
    if o.scale == 1.0 { format!("{}x{}", o.width, o.height) } else { format!("{}x{} scale {}", o.width, o.height, o.scale) }
}

fn session(s: &SessionInfo) -> String {
    let mut line = format!("{} {} apps={} windows={}", s.name, output(&s.output), s.apps, s.windows);
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
        };
        out.push_str(&"  ".repeat(depth));
        out.push_str(&format!("{} {kind} {},{} {}x{}", n.id, n.x, n.y, n.width, n.height));
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

pub fn print(cmd: &Command, v: serde_json::Value) -> Result<(), String> {
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
        Command::Ping | Command::Shutdown => return Ok(()),
    };
    println!("{text}");
    Ok(())
}
