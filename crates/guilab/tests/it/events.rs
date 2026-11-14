//! Event log and delivery: tiers, coalescing, events on the next command, poll/follow,
//! say, timeouts. Human actions go through the hidden `guilab debug` (the viewer's stand-in).

use std::{
    io::{BufRead, BufReader},
    process::Stdio,
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::{Lab, app, have, input::gtk4};

impl Lab {
    /// Runs `guilab ARGS`; returns (exit code, stdout, stderr).
    fn status(&self, args: &[&str]) -> (i32, String, String) {
        self.status_in(std::path::Path::new("/"), args)
    }

    /// `status` with the cwd in `dir` (e.g. a [`Lab::worktree`]).
    fn status_in(&self, dir: &std::path::Path, args: &[&str]) -> (i32, String, String) {
        let out = self.cmd(args).current_dir(dir).output().unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into())
    }

    fn run_in(&self, dir: &std::path::Path, args: &[&str]) -> String {
        let (code, out, err) = self.status_in(dir, args);
        assert_eq!(code, 0, "guilab {args:?} failed: {err}");
        out
    }

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
    let shot = lab.run(&["screenshot"]).split(' ').next().unwrap().to_string();
    // Two pollers on the same key both get the message.
    let polls: Vec<_> = (0..2).map(|_| lab.cmd(&["poll", "--timeout", "20"]).stdout(Stdio::piped()).spawn().unwrap()).collect();
    let mut follow = lab.cmd(&["poll", "--follow"]).stdout(Stdio::piped()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let t = Instant::now();
    lab.run(&["debug", "message", "look\nhere", "--image", &shot, "--note", "1@40,50:this", "--note", "general"]);
    for p in polls {
        let out = p.wait_with_output().unwrap();
        assert!(out.status.success());
        let out = String::from_utf8_lossy(&out.stdout);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].ends_with("human message: \"look\\nhere\""), "{out}");
        assert!(lines[1].starts_with("  image 1: ") && lines[1].contains("msg-"), "{out}");
        assert_eq!(&lines[2..4], ["  annotation on image 1 at pixel 40,50: \"this\"", "  annotation: \"general\""]);
        assert!(lines[4].contains("guilab done"), "{out}");
        let img = lines[1].trim_start_matches("  image 1: ");
        assert_eq!(std::fs::read(img).unwrap(), std::fs::read(&shot).unwrap(), "the image is copied");
    }
    assert!(t.elapsed() < Duration::from_secs(3));
    // --follow: one line per wake event, and it keeps going.
    let mut lines = BufReader::new(follow.stdout.take().unwrap()).lines();
    assert!(lines.next().unwrap().unwrap().contains("human message: \"look\\nhere\" [image 1: "));
    lab.run(&["run", "--", "false"]);
    assert!(lines.next().unwrap().unwrap().contains("app 1 (false) exited with code 1"));
    lab.run(&["debug", "message", "again"]);
    assert!(lines.next().unwrap().unwrap().contains("human message: \"again\""));
    follow.kill().unwrap();
    let _ = follow.wait();
    // Ending the session ends polls with an error.
    lab.run(&["done"]);
    let p = lab.cmd(&["poll"]).stderr(Stdio::piped()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    lab.run(&["end"]);
    let out = p.wait_with_output().unwrap();
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("ended"));
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
    let shot = lab.run(&["screenshot"]).split(' ').next().unwrap().to_string();
    let (ok, _, err) = lab.try_run(&["say", "hi"]);
    assert!(!ok && err.contains("guilab work") && err.contains("guilab done"), "{err}");
    assert_eq!(lab.run(&["work", "checking", "-m", "Is this right?", "--image", &shot]), "working: checking; message posted\n");
    let ev = lab.log_json(&["--kind", "agent"]);
    assert_eq!(kinds(&ev), ["command", "screenshot", "agent_message", "work"], "{ev:?}");
    let img = ev[2]["images"][0].as_str().unwrap();
    assert!(img.contains("/agent-") && std::path::Path::new(img).exists(), "{ev:?}");
    // The agent's own messages and status are log-only: nothing delivered.
    let (code, out, _) = lab.status(&["poll", "--timeout", "0"]);
    assert_eq!((code, out.as_str()), (2, "Timed out: no wake event within 0s (exit code 2).\n"));

    // `done` releases the lock, with or without the token.
    let t = lab.lock();
    let out = lab.run(&["done"]);
    assert!(out.contains("released your lock"), "{out}");
    let (ok, _, err) = lab.try_run(&["-t", &t, "input", "move", "1", "1"]);
    assert!(!ok && err.contains("guilab work"), "{err}");
    let t = lab.lock();
    // Locked and not shared: human input is blocked.
    let (ok, _, err) = lab.try_run(&["debug", "input", "move", "1", "1"]);
    assert!(!ok && err.contains("blocked"), "{err}");
    lab.run(&["debug", "share", "on"]);
    lab.run(&["debug", "input", "move", "1", "1"]);
    let mut p = lab.cmd(&["-t", &t, "poll", "--follow"]).stdout(Stdio::piped()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    lab.run(&["debug", "revoke"]);
    let mut lines = BufReader::new(p.stdout.take().unwrap()).lines();
    let line = lines.next().unwrap().unwrap();
    assert!(line.contains("the human revoked the agent's lock") && line.contains("shared the session"), "{line}");
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
    lab.wait_tree("w", 10, |t| !crate::nodes(t).is_empty());
    // Windows are diffed on the next frame, which may come just after `tree` saw it.
    let deadline = Instant::now() + Duration::from_secs(2);
    let ev = loop {
        let ev = lab.log_json(&["--kind", "window"]);
        if !kinds(&ev).is_empty() || Instant::now() > deadline {
            break ev;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(ev[0]["kind"], "window_map", "{ev:?}");
    assert_eq!(ev[0]["window"], "toplevel");
    // A window the app closed itself says so.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !lab.log_json(&["--kind", "window_unmap"]).iter().any(|e| e["by"] == "app") {
        assert!(Instant::now() < deadline, "{:?}", lab.log_json(&["--kind", "window"]));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(lab.run(&["log"]).contains("closed by its app: \"second window\""));
    // Window changes are passive: reported with the next command. `kill` waits for the app
    // to exit, so it reports the close itself.
    let (_, _, err) = lab.status(&["kill"]);
    assert!(err.contains("window") && err.contains("closed ("), "{err}");
    assert!(kinds(&lab.log_json(&["--kind", "window"])).contains(&"window_unmap"));
}
