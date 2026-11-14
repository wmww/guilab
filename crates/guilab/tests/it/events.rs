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
        let out = self.cmd(args).output().unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into())
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
    // Killed apps are passive: the poll times out (exit 2) and reports it.
    lab.run(&["run", "--", "sleep", "100"]);
    lab.run(&["kill"]);
    std::thread::sleep(Duration::from_millis(300));
    let (code, out, _) = lab.status(&["poll", "--timeout", "0.5"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("no wake event within 0.5s") && out.contains("app 1 (sleep) ended after guilab kill"), "{out}");
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
    assert!(err.contains("human: moved pointer 5,5 → 200,100 (0.2s); double-clicked at 200,100; typed \"Hi there\"; pressed Return; pressed ctrl+s"), "{err}");
    // Delivered once.
    let (_, _, err) = lab.status(&["output"]);
    assert!(err.is_empty(), "{err}");
    // Own actions aren't echoed back; other clients (a lock token vs none) see them.
    let t = lab.lock();
    lab.run(&["-t", &t, "run", "--", "true"]);
    let (_, _, err) = lab.status(&["-t", &t, "ps"]);
    assert!(!err.contains("started"), "{err}");
    let (_, _, err) = lab.status(&["ps"]);
    assert!(err.contains("app 1 started: true"), "{err}");
    lab.run(&["-t", &t, "unlock"]);
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
        assert_eq!(&lines[2..], ["  annotation on image 1 at pixel 40,50: \"this\"", "  annotation: \"general\"", "  (reply with: guilab say \"...\")"]);
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
    let p = lab.cmd(&["poll"]).stderr(Stdio::piped()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    lab.run(&["end"]);
    let out = p.wait_with_output().unwrap();
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("ended"));
}

#[test]
fn say_lock_and_revoke() {
    let lab = Lab::new();
    lab.run(&["new"]);
    let shot = lab.run(&["screenshot"]).split(' ').next().unwrap().to_string();
    lab.run(&["say", "Is this right?", "--image", &shot]);
    let ev = lab.log_json(&["--kind", "say"]);
    let img = ev[0]["images"][0].as_str().unwrap();
    assert!(img.contains("/say-") && std::path::Path::new(img).exists(), "{ev:?}");
    // say is log-only: no wake, nothing delivered.
    let (code, out, _) = lab.status(&["poll", "--timeout", "0.3"]);
    assert_eq!((code, out.as_str()), (2, "Timed out: no wake event within 0.3s (exit code 2).\n"));

    let t = lab.lock();
    // Locked and not shared: human input is blocked.
    let (ok, _, err) = lab.try_run(&["debug", "input", "move", "1", "1"]);
    assert!(!ok && err.contains("blocked"), "{err}");
    lab.run(&["debug", "share", "on"]);
    lab.run(&["debug", "input", "move", "1", "1"]);
    // The token's own cursor starts at the lock.
    let (code, out, _) = lab.status(&["-t", &t, "poll", "--timeout", "0.3"]);
    assert_eq!(code, 2);
    assert!(out.contains("shared the session") && out.contains("human: moved pointer") && !out.contains("session locked"), "{out}");
    let p = lab.cmd(&["-t", &t, "poll", "--timeout", "10"]).stdout(Stdio::piped()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    lab.run(&["debug", "revoke"]);
    let out = String::from_utf8(p.wait_with_output().unwrap().stdout).unwrap();
    assert!(out.contains("the human revoked the agent's lock"), "{out}");
    let (ok, _, err) = lab.try_run(&["-t", &t, "input", "move", "1", "1"]);
    assert!(!ok && err.contains("revoked"), "{err}");
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
    lab.run(&["run", "--", "python3", &app("gtk_app.py")]);
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
    lab.run(&["kill"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !kinds(&lab.log_json(&["--kind", "window"])).contains(&"window_unmap") {
        assert!(Instant::now() < deadline, "no unmap");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Window changes are passive: reported with the next command (`wait_tree` got the map).
    let (_, _, err) = lab.status(&["ps"]);
    assert!(err.contains("window") && err.contains("closed"), "{err}");
}
