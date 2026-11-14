//! End-to-end tests: drive the real `guilab` binary against real apps.
//! Each test gets its own server via a private runtime dir. Tests needing an app that
//! isn't installed are skipped (with a note on stderr).

mod apps;
mod cleanup;
mod events;
mod images;
mod input;
mod sessions;
mod viewer;
mod xwayland;

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};

use serde_json::Value;

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

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_guilab"));
        // No WAYLAND_DISPLAY: a test's `show` must never open on the real desktop.
        c.args(args).env("XDG_RUNTIME_DIR", &self.rt).env_remove("GUILAB_SESSION").env_remove("WAYLAND_DISPLAY").current_dir("/");
        c
    }

    /// Runs `guilab ARGS` with extra environment; returns (success, stdout, stderr).
    pub fn try_run_env(&self, env: &[(&str, &str)], args: &[&str]) -> (bool, String, String) {
        let mut c = self.cmd(args);
        c.envs(env.iter().copied());
        let out = c.output().unwrap();
        (out.status.success(), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into())
    }

    /// Runs `guilab ARGS`; returns (success, stdout, stderr).
    pub fn try_run(&self, args: &[&str]) -> (bool, String, String) {
        let out = self.cmd(args).output().unwrap();
        (out.status.success(), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into())
    }

    /// Runs `guilab ARGS`, panicking on failure; returns stdout.
    pub fn run(&self, args: &[&str]) -> String {
        let (ok, out, err) = self.try_run(args);
        assert!(ok, "guilab {args:?} failed: {err}");
        out
    }

    /// Runs `guilab --json ARGS` and parses the reply.
    pub fn json(&self, args: &[&str]) -> Value {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        serde_json::from_str(&self.run(&a)).unwrap()
    }

    /// Polls `tree` until `pred` holds (or panics after `secs`).
    pub fn wait_tree(&self, session: &str, secs: u64, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let t = self.json(&["-S", session, "tree"]);
            if pred(&t) {
                return t;
            }
            assert!(Instant::now() < deadline, "tree never matched: {t:#}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn path(&self, name: &str) -> String {
        self.rt.join(name).to_string_lossy().into()
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = self.cmd(&["server", "--stop"]).output();
        let _ = std::fs::remove_dir_all(&self.rt);
    }
}

pub fn have(prog: &str) -> bool {
    let found = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
        .unwrap_or(false);
    if !found {
        eprintln!("skipping: {prog} not installed");
    }
    found
}

pub fn app(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/apps").join(name).to_string_lossy().into()
}

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
