//! Event log and delivery: tiers, coalescing, events on the next command, poll/follow,
//! timeouts. Human actions go through the hidden `guilab debug` (the viewer's stand-in).

use std::{
    io::{BufRead, BufReader},
    process::Stdio,
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::*;

impl Lab {
    /// A directory that looks like a git worktree root (a client identity for deliveries).
    fn worktree(&self, name: &str) -> std::path::PathBuf {
        let d = self.rt.join(name);
        std::fs::create_dir_all(d.join(".git")).unwrap();
        d
    }

    fn log_json(&self, args: &[&str]) -> Vec<Value> {
        let mut a = vec!["log", "-n", "0"];
        a.extend_from_slice(args);
        self.json(&a)["events"].as_array().unwrap().clone()
    }
}

fn kinds(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["kind"].as_str().unwrap()).collect()
}

#[test]
fn wake_and_passive() {
    let lab = Lab::new();
    lab.run(&["new"]);
    // Killed apps are passive: the poll times out (exit 2).
    lab.run(&["run", "--", "sleep", "100"]);
    // `kill` waits for the exit, so the exit is reported on its stderr.
    let (_, _, err) = lab.status(&["kill"]);
    assert!(err.contains("app 1 (sleep) ended after guilab kill"), "{err}");
    let (code, out, _) = lab.status(&["poll", "--timeout", "0.5"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("no wake event within 0.5s"), "{out}");
    // An app exiting on its own wakes, with its status and log.
    lab.run(&["run", "--", "sh", "-c", "sleep 0.3; exit 3"]);
    let t = Instant::now();
    let (code, out, _) = lab.status(&["poll", "--timeout", "10"]);
    assert_eq!(code, 0);
    assert!(t.elapsed() < Duration::from_secs(5));
    assert!(out.contains("app 2 (sh) exited with code 3; log: ") && out.contains("app-2.log"), "{out}");
    // Already delivered: it doesn't wake the next poll.
    let (code, _, _) = lab.status(&["poll", "--timeout", "0.3"]);
    assert_eq!(code, 2);
    // Crashes.
    lab.run(&["run", "--", "sh", "-c", "kill -SEGV $$"]);
    assert!(lab.run(&["poll", "--timeout", "10"]).contains("crashed (SIGSEGV)"));

    // A human message wakes; with --passive message it doesn't, but is still reported.
    lab.run(&["debug", "message", "hello"]);
    let (code, out, _) = lab.status(&["poll", "--timeout", "0.3", "--passive", "message"]);
    assert_eq!(code, 2);
    assert!(out.contains("human message: \"hello\""), "{out}");
    // --wake input: human input wakes.
    lab.run(&["debug", "input", "move", "10", "10", "click"]);
    let (code, out, _) = lab.status(&["poll", "--timeout", "1", "--wake", "button"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("human pressed left button at 10,10"), "{out}");
    let (ok, _, err) = lab.try_run(&["poll", "--wake", "nope"]);
    assert!(!ok && err.contains("unknown event kind 'nope'"), "{err}");
}

#[test]
fn delivery_on_next_command() {
    let lab = Lab::new();
    lab.run(&["new", "-s", "640x480"]);
    // Human input: summarized on the next command's stderr, stdout untouched.
    lab.run(&["debug", "input", "move", "5", "5", "move", "200", "100", "200", "click", "click", "type", "Hi there", "key", "Return", "key", "ctrl+s"]);
    let (code, out, err) = lab.status(&["output"]);
    assert_eq!((code, out.as_str()), (0, "640x480\n"));
    assert!(err.starts_with("[guilab events since the session started; not errors]\n"), "{err}");
    // Pointer moves before a click are dropped (the click says where).
    assert!(err.contains("human: double-clicked at 200,100; typed \"Hi there\"; pressed Return; pressed ctrl+s\n"), "{err}");
    // Delivered once.
    let (_, _, err) = lab.status(&["output"]);
    assert!(err.is_empty(), "{err}");
    // Own actions aren't echoed back; other worktrees see them (one cursor per worktree).
    let (a, b) = (lab.worktree("a"), lab.worktree("b"));
    lab.run_in(&a, &["run", "--", "true"]);
    let (_, _, err) = lab.status_in(&a, &["ps"]);
    assert!(!err.contains("started"), "{err}");
    let (_, _, err) = lab.status_in(&b, &["ps"]);
    assert!(err.contains("app 1 started: true"), "{err}");
    // --json: events as one JSON line on stderr.
    lab.run(&["debug", "input", "move", "1", "1"]);
    let out = lab.cmd(&["--json", "output"]).output().unwrap();
    let ev: Value = serde_json::from_slice(&out.stderr).unwrap();
    let last = ev["events"].as_array().unwrap().last().unwrap().clone();
    assert_eq!((last["kind"].as_str(), last["tier"].as_str()), (Some("motion"), Some("passive")), "{ev}");
    // A session's own worktree and outside git share one cursor (an agent changing dirs).
    let c = lab.worktree("c");
    lab.run_in(&c, &["new", "own"]);
    lab.run_in(&c, &["-S", "own", "run", "--", "true"]);
    let (_, _, err) = lab.status(&["-S", "own", "ps"]);
    assert!(!err.contains("started"), "{err}");
}

#[test]
fn coalescing_and_log() {
    let lab = Lab::new();
    lab.run(&["new"]);
    lab.run(&["debug", "input", "move", "0", "0", "move", "300", "300", "300", "scroll", "0", "1", "scroll", "0", "2"]);
    let ev = lab.log_json(&["--kind", "input"]);
    assert_eq!(kinds(&ev), ["motion", "scroll"], "{ev:?}");
    let path = ev[0]["path"].as_array().unwrap();
    assert!(path.len() > 10, "{path:?}");
    assert_eq!(path.last().unwrap()[0], 300);
    let ms = path.last().unwrap()[2].as_i64().unwrap();
    assert!((250..1000).contains(&ms), "{ms}");
    assert_eq!(ev[1]["dy"], 3.0);
    // Gestures are logged once, with their totals; a tap as one phrase.
    lab.run(&["debug", "input", "move", "100", "100", "pinch", "2", "swipe", "300", "0", "tap", "50", "60"]);
    let (_, _, err) = lab.status(&["ps"]);
    assert!(err.contains("2-finger pinch to 2x; 3-finger swipe by 300,0; tapped at 50,60"), "{err}");
    // Commands are logged with their args; filters.
    lab.run(&["ps"]);
    lab.run(&["tree"]);
    let out = lab.run(&["log"]);
    assert!(out.contains("agent ran: guilab tree") && out.contains("human moved pointer 0,0 → 300,300"), "{out}");
    let out = lab.run(&["log", "--kind", "command", "-n", "1"]);
    assert!(out.starts_with("(") && out.contains("earlier events not shown") && out.ends_with("agent ran: guilab tree\n"), "{out}");
    let all = lab.log_json(&[]);
    let first = all[0]["seq"].as_u64().unwrap();
    assert_eq!(lab.log_json(&["--after", &first.to_string()]).len(), all.len() - 1);
    assert_eq!(lab.log_json(&["--since", "1h"]).len(), all.len());
    assert!(lab.log_json(&["--since", "23:59:59"]).is_empty() || all.is_empty());
}

#[test]
fn poll_blocks_and_follows() {
    let lab = Lab::new();
    lab.run(&["new"]);
    let shot = lab.json(&["screenshot"])["image"].as_str().unwrap().to_string();
    let drawn = lab.json(&["image", "make", "--from", &shot, "blue box 40,50 100x60", "text 150,50 \"this\""]);
    let drawn = drawn["id"].as_str().unwrap();
    // Two pollers on the same key both get the message.
    let polls: Vec<_> = (0..2).map(|_| lab.cmd(&["poll", "--timeout", "20"]).stdout(Stdio::piped()).spawn().unwrap()).collect();
    let mut follow = lab.cmd(&["poll", "--follow"]).stdout(Stdio::piped()).spawn().unwrap();
    lab.wait_polls(3);
    let t = Instant::now();
    lab.run(&["debug", "message", "look\nhere", "--image", drawn]);
    for p in polls {
        let out = p.wait_with_output().unwrap();
        assert!(out.status.success());
        let out = String::from_utf8_lossy(&out.stdout);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].ends_with("human message: \"look\\nhere\""), "{out}");
        assert_eq!(lines[1], format!("  image {drawn} 1280x720, parent {shot} (your screenshot)"), "{out}");
        assert!(lines[2].starts_with("    1 raster r") && lines[2].contains("0,0 1280x720 (screenshot "), "{out}");
        assert_eq!(lines[3], "    2 blue box 40,50 100x60", "{out}");
        assert!(lines[4].starts_with("    3 red text 150,50 \"this\" (box "), "{out}");
        let hint = format!("  (see what the marks point at: guilab image {drawn} --crop '");
        assert!(lines[5].starts_with(&hint), "{out}");
        assert!(lines[6].contains("guilab done"), "{out}");
    }
    // The hint renders the marks' area.
    let crop = lab.run(&["image", drawn, "--crop", "18,28 300x104"]);
    let png = crop.lines().next().unwrap();
    assert_eq!(read_png(png).0, 300);
    assert!(crop.contains("PNG shows canvas 18,28 300x104 at 1x"), "{crop}");
    assert!(t.elapsed() < Duration::from_secs(3));
    // --follow: one line per wake event, and it keeps going.
    let mut lines = BufReader::new(follow.stdout.take().unwrap()).lines();
    assert!(lines.next().unwrap().unwrap().contains(&format!("human message: \"look\\nhere\" [image {drawn}: 3 elements]")));
    lab.run(&["run", "--", "false"]);
    assert!(lines.next().unwrap().unwrap().contains("app 1 (false) exited with code 1"));
    lab.run(&["debug", "message", "again", "--draw", "green ellipse 5,5 20x10"]);
    let line = lines.next().unwrap().unwrap();
    assert!(line.contains("human message: \"again\" [image ") && line.contains(": 2 elements]"), "{line}");
    lab.run(&["debug", "message", "and this"]);
    let line = lines.next().unwrap().unwrap();
    assert!(line.contains("human message: \"and this\" [their screen: image "), "{line}");
    follow.kill().unwrap();
    let _ = follow.wait();
    lab.wait_polls(0); // the server noticed the follower is gone
    // Ending the session ends polls with an error.
    lab.run(&["done"]);
    let p = lab.cmd(&["poll"]).stderr(Stdio::piped()).spawn().unwrap();
    lab.wait_polls(1);
    lab.run(&["end"]);
    let out = p.wait_with_output().unwrap();
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("ended"));
}

/// A `poll --follow` reader that stops reading (piped into a paused process) must not
/// stall the server: its replies wait in the server, not in a blocking write.
#[test]
fn stalled_follower_doesnt_block() {
    let lab = Lab::new();
    lab.run(&["new"]);
    let mut follow = lab.cmd(&["poll", "--follow"]).stdout(Stdio::null()).spawn().unwrap();
    lab.wait_polls(1);
    let pid = follow.id().to_string();
    assert!(std::process::Command::new("kill").args(["-STOP", &pid]).status().unwrap().success());
    // Enough to fill the socket buffers several times over.
    let big = "x".repeat(60_000);
    let t = Instant::now();
    for _ in 0..12 {
        lab.run(&["debug", "message", &big]);
    }
    let took = t.elapsed();
    follow.kill().unwrap();
    let _ = follow.wait();
    assert!(took < Duration::from_secs(5), "{took:?}");
}

/// The lost-message bug: a human message must survive an agent dropping a command's
/// stderr. Replies only say one is waiting; it's shown (once) by a poll.
#[test]
fn messages_wait_for_poll() {
    let lab = Lab::new();
    lab.run(&["new"]);
    lab.run(&["work", "building"]);
    lab.run(&["debug", "message", "stop, wrong window"]);
    for _ in 0..2 {
        let (_, out, err) = lab.status(&["ps"]);
        assert!(!out.contains("wrong window") && !err.contains("wrong window"), "{err}");
        assert!(err.contains("[guilab: a human message waiting since ") && err.contains("read it now: guilab poll]"), "{err}");
    }
    // `done` won't answer before it's read.
    let (ok, _, err) = lab.try_run(&["done", "all fixed"]);
    assert!(!ok && err.contains("haven't read") && err.contains("guilab poll"), "{err}");
    // A poll returns it at once, even while working, and marks the agent on it.
    let t = Instant::now();
    let (code, out, _) = lab.status(&["poll", "--timeout", "10"]);
    assert_eq!(code, 0, "{out}");
    assert!(t.elapsed() < Duration::from_secs(2));
    assert!(out.contains("human message: \"stop, wrong window\"") && out.contains("guilab done"), "{out}");
    assert!(lab.run(&["ls"]).contains("working=\"read your message\""));
    let (_, _, err) = lab.status(&["ps"]);
    assert!(!err.contains("waiting"), "{err}");
    // Blocking polls need `done`; a zero timeout just checks.
    let (code, _, err) = lab.status(&["poll", "--timeout", "10"]);
    assert!(code == 1 && err.contains("still marked working") && err.contains("guilab done"), "{err}");
    assert_eq!(lab.status(&["poll", "--timeout", "0"]).0, 2);
    let out = lab.run(&["done", "fixed"]);
    assert!(out.contains("marked done") && out.contains("guilab poll"), "{out}");
    // Read once: it doesn't come back.
    assert_eq!(lab.status(&["poll", "--timeout", "0.3"]).0, 2);
    assert!(!lab.run(&["ls"]).contains("working="));
}

#[test]
fn work_done_lock_and_revoke() {
    let lab = Lab::new();
    lab.run(&["new"]);
    let shot = lab.json(&["screenshot"])["image"].as_str().unwrap().to_string();
    let (ok, _, err) = lab.try_run(&["say", "hi"]);
    assert!(!ok && err.contains("guilab work") && err.contains("guilab done"), "{err}");
    assert_eq!(lab.run(&["work", "checking", "-m", "Is this right?", "--image", &shot]), "working: checking; message posted\n");
    let ev = lab.log_json(&["--kind", "agent"]);
    assert_eq!(kinds(&ev), ["command", "screenshot", "agent_message", "work"], "{ev:?}");
    assert_eq!(ev[2]["images"][0]["id"], shot.as_str(), "{ev:?}");
    // A guilab rendering isn't a picture to attach: the id is.
    let png = lab.json(&["image", &shot])["render"]["path"].as_str().unwrap().to_string();
    let (ok, _, err) = lab.try_run(&["work", "checking", "-m", "this", "--image", &png]);
    assert!(!ok && err.contains(&format!("--image {shot}")), "{err}");
    // The agent's own messages and status are log-only: nothing delivered.
    let (code, out, _) = lab.status(&["poll", "--timeout", "0"]);
    assert_eq!((code, out.as_str()), (2, "Timed out: no wake event within 0s (exit code 2).\n"));

    // `done` releases the lock, with or without the token. A bad image fails it before
    // anything changed: still locked and working.
    let t = lab.lock();
    let (ok, _, err) = lab.try_run(&["done", "bye", "--image", "/nonexistent.png"]);
    assert!(!ok && err.contains("/nonexistent.png"), "{err}");
    lab.input(&t, &["move", "1", "1"]);
    let out = lab.run(&["done"]);
    assert!(out.contains("released your lock"), "{out}");
    let (ok, _, err) = lab.try_run(&["-t", &t, "input", "move", "1", "1"]);
    assert!(!ok && err.contains("guilab work"), "{err}");
    let t = lab.lock();
    // Locked: human input is blocked.
    let (ok, _, err) = lab.try_run(&["debug", "input", "move", "1", "1"]);
    assert!(!ok && err.contains("blocked"), "{err}");
    let mut p = lab.cmd(&["-t", &t, "poll", "--follow"]).stdout(Stdio::piped()).spawn().unwrap();
    lab.wait_polls(1);
    lab.run(&["debug", "revoke"]);
    let mut lines = BufReader::new(p.stdout.take().unwrap()).lines();
    let line = lines.next().unwrap().unwrap();
    assert!(line.contains("the human revoked the agent's lock"), "{line}");
    let (ok, _, err) = lab.try_run(&["-t", &t, "input", "move", "1", "1"]);
    assert!(!ok && err.contains("revoked"), "{err}");
    let (ok, _, err) = lab.try_run(&["-t", &t, "done"]);
    assert!(ok, "{err}");
    p.kill().unwrap();
    let _ = p.wait();
}

#[test]
fn expiry_warning() {
    let lab = Lab::new();
    lab.run(&["new", "--ttl", "4"]);
    // Warned 2 s before the end (half the TTL, at most 60 s).
    let t = Instant::now();
    let (code, out, _) = lab.status(&["poll", "--timeout", "10"]);
    assert_eq!(code, 0, "{out}");
    // Whole seconds, checked once a second.
    assert!(out.contains("ends in 2s from inactivity") || out.contains("ends in 1s from inactivity"), "{out}");
    assert!(t.elapsed() >= Duration::from_millis(1500), "{:?}", t.elapsed());
}

#[test]
fn window_events() {
    if !have("python3") || !gtk4() {
        return;
    }
    let lab = Lab::new();
    lab.run(&["new", "w"]);
    lab.run(&["run", "--", "python3", &app("gtk_app.py"), "--second", "--close-second"]);
    lab.wait_tree("w", 10, |t| !nodes(t).is_empty());
    // Windows are diffed on the next frame, which may come just after `tree` saw it.
    let ev = wait_until("a window event", 10, || {
        let ev = lab.log_json(&["--kind", "window"]);
        if ev.is_empty() { Err("none".into()) } else { Ok(ev) }
    });
    assert_eq!(ev[0]["kind"], "window_map", "{ev:?}");
    assert_eq!(ev[0]["window"], "toplevel");
    // A window the app closed itself says so.
    wait_until("a window closed by its app", 10, || {
        let ev = lab.log_json(&["--kind", "window"]);
        if ev.iter().any(|e| e["kind"] == "window_unmap" && e["by"] == "app") { Ok(()) } else { Err(format!("{ev:?}")) }
    });
    assert!(lab.run(&["log"]).contains("closed by its app: \"second window\""));
    // Window changes are passive: reported with the next command. `kill` waits for the app
    // to exit, so it reports the close itself.
    let t = Instant::now();
    let (_, _, err) = lab.status(&["kill"]);
    assert!(err.contains("window") && err.contains("closed ("), "{err}\nkill took {:?}; log:\n{}", t.elapsed(), lab.run(&["log"]));
    assert!(kinds(&lab.log_json(&["--kind", "window"])).contains(&"window_unmap"));
}
