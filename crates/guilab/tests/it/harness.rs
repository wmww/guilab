//! Shared test harness: a private server per test (`Lab`), one way to run the CLI,
//! `wait_until` for every wait on something asynchronous, app detection and skips, PNG
//! and tree helpers.
//!
//! Waiting: never sleep and then assert. Wait for what the test needs to see (an app log
//! line, a `tree` shape, `clip get`, a process gone, a pending poll) with `wait_until` or a
//! helper built on it; it returns as soon as the condition holds and its timeout only
//! matters when something is broken, so make timeouts generous.

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};

use guilab_proto::{Client, Request, ServerInfo, Target};
use serde_json::Value;

/// Shortest wait timeout: a wait returns as soon as its condition holds, so the timeout
/// only matters when something is broken, and a loaded machine (other agents building)
/// can make a GTK app take seconds to react.
const MIN_WAIT: u64 = 15;

/// Polls `f` until it returns `Ok`, every 30 ms; panics after `secs` (at least
/// `MIN_WAIT`) with `what` and the last `Err` (the state seen, for the failure message).
#[track_caller]
pub fn wait_until<T>(what: &str, secs: u64, mut f: impl FnMut() -> Result<T, String>) -> T {
    let secs = secs.max(MIN_WAIT);
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        match f() {
            Ok(v) => return v,
            Err(state) if Instant::now() >= deadline => panic!("timed out after {secs} s waiting for {what}; last seen:\n{state}"),
            Err(_) => std::thread::sleep(Duration::from_millis(30)),
        }
    }
}

/// A finished `guilab` command.
pub struct Out {
    pub code: i32,
    pub out: String,
    pub err: String,
}

/// A private guilab server (autostarted on first use) in its own runtime dir.
pub struct Lab {
    pub rt: PathBuf,
}

static N: AtomicU32 = AtomicU32::new(0);

impl Lab {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Lab {
        // Keep it short: Unix socket paths are limited to 108 bytes.
        let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let rt = base.join(format!("glt-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&rt);
        std::fs::create_dir_all(&rt).unwrap();
        Lab { rt }
    }

    /// `guilab ARGS` as a command (cwd `/`), for spawning.
    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_guilab"));
        // No WAYLAND_DISPLAY: a test's `show` must never open on the real desktop.
        c.args(args).env("XDG_RUNTIME_DIR", &self.rt).env_remove("GUILAB_SESSION").env_remove("WAYLAND_DISPLAY").current_dir("/");
        c
    }

    /// Runs `guilab ARGS` with extra environment and optionally another cwd.
    pub fn exec_with(&self, env: &[(&str, &str)], dir: Option<&Path>, args: &[&str]) -> Out {
        let mut c = self.cmd(args);
        c.envs(env.iter().copied());
        if let Some(d) = dir {
            c.current_dir(d);
        }
        let o = c.output().unwrap();
        Out { code: o.status.code().unwrap_or(-1), out: String::from_utf8_lossy(&o.stdout).into(), err: String::from_utf8_lossy(&o.stderr).into() }
    }

    /// Runs `guilab ARGS`; returns (success, stdout, stderr).
    pub fn try_run(&self, args: &[&str]) -> (bool, String, String) {
        self.try_run_env(&[], args)
    }

    pub fn try_run_env(&self, env: &[(&str, &str)], args: &[&str]) -> (bool, String, String) {
        let o = self.exec_with(env, None, args);
        (o.code == 0, o.out, o.err)
    }

    /// Runs `guilab ARGS`; returns (exit code, stdout, stderr).
    pub fn status(&self, args: &[&str]) -> (i32, String, String) {
        self.status_in(Path::new("/"), args)
    }

    /// `status` with the cwd in `dir`.
    pub fn status_in(&self, dir: &Path, args: &[&str]) -> (i32, String, String) {
        let o = self.exec_with(&[], Some(dir), args);
        (o.code, o.out, o.err)
    }

    /// Runs `guilab ARGS`, panicking on failure; returns stdout.
    #[track_caller]
    pub fn run(&self, args: &[&str]) -> String {
        self.run_in(Path::new("/"), args)
    }

    #[track_caller]
    pub fn run_in(&self, dir: &Path, args: &[&str]) -> String {
        let o = self.exec_with(&[], Some(dir), args);
        assert!(o.code == 0, "guilab {args:?} failed: {}", o.err);
        o.out
    }

    /// Runs `guilab --json ARGS` and parses the reply.
    #[track_caller]
    pub fn json(&self, args: &[&str]) -> Value {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        serde_json::from_str(&self.run(&a)).unwrap()
    }

    /// The server's `ping` reply (renderer, viewer, pending requests).
    pub fn info(&self) -> ServerInfo {
        let mut c = Client::connect(&self.rt.join("guilab/control.sock")).unwrap();
        c.call_as(&Request { proto: guilab_proto::PROTO, target: Target::default(), cmd: guilab_proto::GlobalCmd::Ping.into() }).unwrap()
    }

    /// Waits until `n` polls are pending in the server (spawned pollers registered).
    #[track_caller]
    pub fn wait_polls(&self, n: usize) {
        wait_until(&format!("{n} pending polls"), 10, || {
            let p = self.info().pending;
            if p.polls == n { Ok(()) } else { Err(format!("{p:?}")) }
        });
    }

    /// Polls `tree` until `pred` holds.
    #[track_caller]
    pub fn wait_tree(&self, session: &str, secs: u64, pred: impl Fn(&Value) -> bool) -> Value {
        wait_until("a matching tree", secs, || {
            let t = self.json(&["-S", session, "tree"]);
            if pred(&t) { Ok(t) } else { Err(format!("{t:#}")) }
        })
    }

    /// Polls `clip get` until it returns `want`. Copying is asynchronous (the app
    /// announces the selection whenever it gets to it).
    #[track_caller]
    pub fn wait_clip(&self, want: &str, secs: u64) {
        wait_until(&format!("clipboard {want:?}"), secs, || {
            let (ok, out, err) = self.try_run(&["--json", "clip", "get"]);
            let got = if ok { serde_json::from_str::<Value>(&out).unwrap() } else { Value::String(err) };
            if got == want { Ok(()) } else { Err(got.to_string()) }
        });
    }

    /// `guilab work`, then `guilab lock`; returns the token.
    #[track_caller]
    pub fn lock(&self) -> String {
        self.run(&["work", "testing"]);
        self.json(&["lock"])["token"].as_str().unwrap().to_string()
    }

    /// `guilab -t TOKEN input ACTIONS...`
    #[track_caller]
    pub fn input(&self, token: &str, actions: &[&str]) -> Value {
        let mut a = vec!["-t", token, "input"];
        a.extend_from_slice(actions);
        self.json(&a)
    }

    pub fn path(&self, name: &str) -> String {
        self.rt.join(name).to_string_lossy().into()
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = self.cmd(&["server", "--stop"]).output();
        // A failed test keeps its runtime dir: `guilab/server.log` (run the tests with e.g.
        // `GUILAB_LOG=debug`), app logs, screenshots.
        if std::thread::panicking() {
            eprintln!("kept {} for inspection", self.rt.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.rt);
        }
    }
}

// ------------------------------------------------------------------ apps and skips

/// Reports a skipped test (missing app). With `GUILAB_REQUIRE_APPS=1` it fails instead,
/// so a machine missing GTK, Qt or Xwayland can't pass by running a fraction of the suite.
pub fn skip(why: &str) -> bool {
    assert!(std::env::var_os("GUILAB_REQUIRE_APPS").is_none_or(|v| v != "1"), "GUILAB_REQUIRE_APPS=1 but {why}");
    eprintln!("skipping: {why}");
    false
}

pub fn have(prog: &str) -> bool {
    let found = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()));
    found || skip(&format!("{prog} not installed"))
}

pub fn has_python_module(check: &str) -> bool {
    let ok = Command::new("python3").args(["-c", check]).status().is_ok_and(|s| s.success());
    ok || skip(&format!("python3 can't run `{check}`"))
}

pub fn gtk4() -> bool {
    has_python_module("import gi; gi.require_version('Gtk','4.0'); from gi.repository import Gtk")
}

pub fn qt6() -> bool {
    has_python_module("import PySide6.QtWidgets")
}

/// Xwayland and the Tk test app.
pub fn xwayland() -> bool {
    have("Xwayland") && has_python_module("import tkinter")
}

pub fn app(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/apps").join(name).to_string_lossy().into()
}

/// An app's log, read until it contains `needle`.
#[track_caller]
pub fn wait_log(log: &str, needle: &str, secs: u64) -> String {
    wait_until(&format!("{needle:?} in the app log"), secs, || {
        let s = std::fs::read_to_string(log).unwrap_or_default();
        if s.contains(needle) { Ok(s) } else { Err(s) }
    })
}

/// Center of a widget from the app's `pos NAME X Y W H` line, in output pixels.
pub fn center(log: &str, name: &str, scale: f64) -> (String, String) {
    let line = log.lines().find(|l| l.starts_with(&format!("pos {name} "))).unwrap_or_else(|| panic!("no pos for {name}"));
    let v: Vec<f64> = line.split(' ').skip(2).map(|x| x.parse().unwrap()).collect();
    let f = |c: f64| format!("{}", (c * scale).round());
    (f(v[0] + v[2] / 2.0), f(v[1] + v[3] / 2.0))
}

pub fn alive(pid: u64) -> bool {
    // Zombies count as dead.
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| !s.contains(") Z "))
}

/// Waits until none of `pids` is alive.
#[track_caller]
pub fn wait_dead(pids: &[u64], what: &str) {
    wait_until(what, 10, || {
        let left: Vec<u64> = pids.iter().copied().filter(|p| alive(*p)).collect();
        if left.is_empty() { Ok(()) } else { Err(format!("still alive: {left:?}")) }
    });
}

pub fn pgrep(pattern: &str) -> Vec<u64> {
    let out = Command::new("pgrep").args(["-f", pattern]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect()
}

// ------------------------------------------------------------------ images and trees

/// Decodes a PNG: (width, height, RGB(A) bytes, channels).
pub fn read_png(path: &str) -> (u32, u32, Vec<u8>, usize) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf, info.color_type.samples())
}

/// Fraction of pixels that aren't (near) black.
pub fn lit_fraction(path: &str) -> f64 {
    let (_, _, px, ch) = read_png(path);
    let n = px.len() / ch;
    let lit = px.chunks(ch).filter(|p| p[..3].iter().any(|c| *c > 24)).count();
    lit as f64 / n as f64
}

/// All nodes of a tree reply, depth first.
pub fn nodes(tree: &Value) -> Vec<&Value> {
    fn walk<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
        for n in v.as_array().into_iter().flatten() {
            out.push(n);
            if let Some(c) = n.get("children") {
                walk(c, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, &mut out);
    out
}

pub fn rect(n: &Value) -> (i64, i64, i64, i64) {
    let g = |k: &str| n[k].as_i64().unwrap();
    (g("x"), g("y"), g("width"), g("height"))
}
