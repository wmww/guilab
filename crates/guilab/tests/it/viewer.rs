//! The viewer window, shown on a headless host: a second guilab server's session plays the
//! human's desktop. The test drives the host session's pointer (as an agent there), which
//! the viewer turns into human input in the viewed session.

use std::time::{Duration, Instant};

use guilab_proto::{Client, Command, Request, ServerInfo, Target};

use crate::{
    Lab, app,
    input::{center, gtk4, wait_log},
};

impl Lab {
    fn info(&self) -> ServerInfo {
        let mut c = Client::connect(&self.rt.join("guilab/control.sock")).unwrap();
        c.call_as(&Request { target: Target::default(), cmd: Command::Ping }).unwrap()
    }

    /// `guilab show` on `host`'s session socket.
    fn show(&self, host: &str) -> (bool, String, String) {
        self.try_run_env(&[("WAYLAND_DISPLAY", host)], &["show"])
    }
}

/// A host server with one session; returns (lab, its socket path, lock token). None if the
/// machine can't run the viewer (no GLES).
fn host() -> Option<(Lab, String, String)> {
    let h = Lab::new();
    let info = h.json(&["new", "host", "-s", "1200x800"]);
    if !h.info().renderer.starts_with("gles") {
        eprintln!("skipping: the viewer needs GLES");
        return None;
    }
    let sock = info["wayland_display"].as_str().unwrap().to_string();
    let t = h.lock();
    Some((h, sock, t))
}

/// A viewed server running gtk_input.py in session "app"; returns (lab, app log).
fn viewed(env: &[(&str, &str)]) -> (Lab, String) {
    let v = Lab::new();
    // The server autostarts with this command's environment.
    let (ok, _, err) = v.try_run_env(env, &["new", "app", "-s", "800x600"]);
    assert!(ok, "{err}");
    let p = v.json(&["run", "--", "python3", &app("gtk_input.py")]);
    let log = p["log"].as_str().unwrap().to_string();
    wait_log(&log, "ready", 20);
    (v, log)
}

/// Host pixel -> viewed-session pixel mapping, found by moving the host pointer to two
/// points and reading the viewed session's human motion events (the view may be zoomed).
struct Mapping {
    /// Host = origin + session * scale.
    origin: (f64, f64),
    scale: f64,
}

fn mapping(h: &Lab, t: &str, v: &Lab) -> Mapping {
    // Waits for the session's latest pointer position to differ from `prev`.
    let probe = |x: i64, y: i64, prev: Option<(f64, f64)>| -> (f64, f64) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            h.input(t, &["move", &x.to_string(), &y.to_string()]);
            std::thread::sleep(Duration::from_millis(100));
            let log = v.json(&["log", "--kind", "motion", "-n", "1"]);
            if let Some(p) = log["events"][0]["path"].as_array().and_then(|p| p.last()) {
                let at = (p[0].as_f64().unwrap(), p[1].as_f64().unwrap());
                if Some(at) != prev {
                    return at;
                }
            }
            assert!(Instant::now() < deadline, "no human motion reached the session: {log}\n{}", v.run(&["log"]));
        }
    };
    let (ax, ay) = probe(400, 300, None);
    let (bx, _) = probe(700, 300, Some((ax, ay)));
    let scale = 300.0 / (bx - ax);
    Mapping { origin: (400.0 - ax * scale, 300.0 - ay * scale), scale }
}

fn click_button(h: &Lab, t: &str, m: &Mapping, log: &str) {
    let (bx, by) = center(&std::fs::read_to_string(log).unwrap(), "button", 1.0);
    let x = (m.origin.0 + bx.parse::<f64>().unwrap() * m.scale).round() as i64;
    let y = (m.origin.1 + by.parse::<f64>().unwrap() * m.scale).round() as i64;
    h.input(t, &["move", &x.to_string(), &y.to_string(), "click"]);
}

fn clicks(log: &str) -> usize {
    std::fs::read_to_string(log).unwrap().lines().filter(|l| l.starts_with("clicked")).count()
}

fn click_through(env: &[(&str, &str)], buffers: &str) {
    let Some((h, sock, t)) = host() else { return };
    if !gtk4() {
        return;
    }
    let (v, log) = viewed(env);
    let (ok, out, err) = v.show(&sock);
    assert!(ok && out.contains("viewer shown"), "{out}{err}");
    let info = v.info().viewer.expect("viewer info");
    assert_eq!(info.buffers, buffers);

    let m = mapping(&h, &t, &v);
    click_button(&h, &t, &m, &log);
    wait_log(&log, "clicked 1", 5);
    // Moving off the image gives the app a pointer leave (host 600,780: the chat box).
    h.input(&t, &["move", "600", "780"]);
    wait_log(&log, "pointer left", 5);
    // A pinch over the app goes to the app (with "Use the app" selected), not the view.
    let (zx, zy) = center(&std::fs::read_to_string(&log).unwrap(), "zoom", 1.0);
    let x = (m.origin.0 + zx.parse::<f64>().unwrap() * m.scale).round() as i64;
    let y = (m.origin.1 + zy.parse::<f64>().unwrap() * m.scale).round() as i64;
    h.input(&t, &["move", &x.to_string(), &y.to_string(), "sleep", "100", "pinch", "2"]);
    wait_log(&log, "zoom end", 5);

    // Locked by an agent: the human's clicks are dropped, until shared.
    let vt = v.lock();
    click_button(&h, &t, &m, &log);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(clicks(&log), 1, "a click got through the lock");
    v.run(&["debug", "share", "on"]);
    click_button(&h, &t, &m, &log);
    wait_log(&log, "clicked 2", 5);
    v.run(&["-t", &vt, "unlock"]);
}

#[test]
fn click_through_dmabuf() {
    click_through(&[], "dmabuf");
}

#[test]
fn click_through_shm() {
    click_through(&[("GUILAB_VIEWER_SHM", "1")], "shm");
}

/// Showing and hiding, also mid-connect, never touches sessions.
#[test]
fn show_hide_cycles() {
    let Some((h, sock, _t)) = host() else { return };
    let v = Lab::new();
    v.run(&["new", "app", "-s", "640x480"]);
    let p = v.json(&["run", "--", "sleep", "600"]);
    for i in 0..15 {
        let (ok, _, err) = v.show(&sock);
        assert!(ok, "show {i}: {err}");
        assert!(v.info().viewer.is_some_and(|i| i.frames > 0));
        v.run(&["hide"]);
        assert!(v.info().viewer.is_none());
    }
    // Hide while connecting: the pending show fails, nothing else happens.
    for _ in 0..5 {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_guilab"));
        c.args(["show"]).env("XDG_RUNTIME_DIR", &v.rt).env("WAYLAND_DISPLAY", &sock).current_dir("/");
        let child = c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        v.run(&["hide"]);
        child.wait_with_output().unwrap();
    }
    v.run(&["hide"]);
    let ps = v.json(&["ps"]);
    assert_eq!(ps[0]["id"], p["id"]);
    assert_eq!(ps[0]["state"], "running", "{ps}");
    let log = v.json(&["log"]);
    let kinds: Vec<&str> = log["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert!(kinds.iter().all(|k| matches!(*k, "command" | "app_start")), "the viewer touched the session: {kinds:?}");
    // The host saw windows come and go, and none is left.
    h.wait_tree("host", 5, |t| t.as_array().unwrap().is_empty());
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
}

/// The host compositor dying closes the viewer only.
#[test]
fn host_killed() {
    let Some((h, sock, _t)) = host() else { return };
    let v = Lab::new();
    v.run(&["new", "app", "-s", "640x480"]);
    v.run(&["run", "--", "sleep", "600"]);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    let pid = h.info().pid;
    rustix::process::kill_process(rustix::process::Pid::from_raw(pid as i32).unwrap(), rustix::process::Signal::KILL).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while v.info().viewer.is_some() {
        assert!(Instant::now() < deadline, "viewer still shown after the host died");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(v.json(&["ps"])[0]["state"], "running");
    v.run(&["screenshot", "--no-settle"]);
    let (ok, _, _) = v.show(&sock);
    assert!(!ok, "show on a dead host worked");
}

#[test]
fn show_refuses_own_session() {
    let v = Lab::new();
    let info = v.json(&["new", "app"]);
    if !v.info().renderer.starts_with("gles") {
        return;
    }
    let (ok, _, err) = v.show(info["wayland_display"].as_str().unwrap());
    assert!(!ok && err.contains("its own sessions"), "{err}");
    let (ok, _, err) = v.try_run(&["show"]);
    assert!(!ok && err.contains("WAYLAND_DISPLAY"), "{err}");
}

/// Drawing and chat, end to end: the agent's `say` reaches the feed; the human draws a box
/// and a text note, types a message and sends it; `poll` gets the image and the note.
/// Positions are the viewer's layout in a 1200x800 window (toolbar at the left, message
/// box at the bottom, the 800x600 session fitted in between).
#[test]
fn draw_and_send() {
    let Some((h, sock, t)) = host() else { return };
    let v = Lab::new();
    v.run(&["new", "app", "-s", "800x600"]);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    let shot = v.path("shot.png");
    v.run(&["screenshot", "--no-settle", "-o", &shot]);
    v.run(&["say", "here's the app", "--image", &shot]);

    let step = |actions: &[&str]| {
        h.input(&t, actions);
        std::thread::sleep(Duration::from_millis(150));
    };
    step(&["move", "19", "133", "click"]); // box tool
    step(&["move", "500", "300", "down", "move", "700", "400", "100", "up"]);
    step(&["move", "19", "262", "click"]); // text tool
    step(&["move", "520", "330", "click"]);
    step(&["type", "label", "key", "Return"]);
    step(&["move", "600", "777", "click"]); // message box
    step(&["type", "fix it", "key", "Return"]);
    let (_, out, err) = v.try_run(&["poll", "--timeout", "10"]);
    assert!(out.contains("human message: \"fix it\""), "{out}{err}");
    assert!(out.contains("red text annotation on image 1 at pixel") && out.contains("\"label\""), "{out}");
    let img = out.lines().find_map(|l| l.trim().strip_prefix("image 1: ")).expect("an image");
    let (w, hh, px, ch) = crate::read_png(img);
    assert_eq!((w, hh), (800, 600));
    let red = px.chunks(ch).filter(|p| p[0] > 180 && p[1] < 80 && p[2] < 80).count();
    assert!(red > 300, "the box isn't in the image ({red} red pixels)");
    assert!(v.info().viewer.is_some(), "the viewer survived");
}

/// A key the human holds repeats in the viewer's text boxes (the host session turns client
/// key repeat on for human keys; the viewer repeats keys for egui itself).
#[test]
fn held_key_repeats() {
    let Some((h, sock, t)) = host() else { return };
    h.run(&["-t", &t, "unlock"]);
    let v = Lab::new();
    v.run(&["new", "app", "-s", "800x600"]);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    h.run(&["debug", "input", "move", "600", "777", "sleep", "100", "click"]); // message box
    std::thread::sleep(Duration::from_millis(200));
    h.run(&["debug", "input", "keydown", "x", "sleep", "1200", "keyup", "x", "key", "Return"]);
    let (_, out, err) = v.try_run(&["poll", "--timeout", "10"]);
    let n = out.lines().find_map(|l| l.split("human message: \"").nth(1)).map(|m| m.chars().take_while(|c| *c == 'x').count());
    assert!(n.is_some_and(|n| n >= 5), "expected repeats: {out}{err}");
}

/// The host shows the app's own cursor over the app: a named shape, a client cursor
/// surface (a red square), or none; and "not allowed" while an agent holds the lock.
#[test]
fn app_cursors() {
    let Some((h, sock, t)) = host() else { return };
    if !gtk4() {
        return;
    }
    let v = Lab::new();
    v.run(&["new", "app", "-s", "800x600"]);
    let log = v.json(&["run", "--", "python3", &app("gtk_cursors.py")])["log"].as_str().unwrap().to_string();
    let applog = wait_log(&log, "ready", 20);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    let m = mapping(&h, &t, &v);
    let shot = h.path("cursor.png");
    // The host's pixels around the pointer, after moving it over `area` (60 px below the
    // label's center, off its text).
    let look = |area: &str| -> Vec<[u8; 3]> {
        let (x, y) = center(&applog, area, 1.0);
        let x = (m.origin.0 + x.parse::<f64>().unwrap() * m.scale).round() as i64;
        let y = (m.origin.1 + (y.parse::<f64>().unwrap() + 60.0) * m.scale).round() as i64;
        h.input(&t, &["move", &(x - 5).to_string(), &y.to_string(), "move", &x.to_string(), &y.to_string()]);
        std::thread::sleep(Duration::from_millis(300));
        h.run(&["screenshot", "--cursor", "-o", &shot]);
        let (w, _, px, ch) = crate::read_png(&shot);
        let mut out = Vec::new();
        for dy in -12..=12 {
            for dx in -12..=12 {
                let i = (((y + dy) * w as i64 + x + dx) as usize) * ch;
                out.push([px[i], px[i + 1], px[i + 2]]);
            }
        }
        out
    };
    let white = |p: &Vec<[u8; 3]>| p.iter().filter(|c| c.iter().all(|v| *v > 220)).count();
    let red = |p: &Vec<[u8; 3]>| p.iter().filter(|c| c[0] > 200 && c[1] < 50 && c[2] < 50).count();
    let named = look("named");
    assert!(white(&named) > 10 && red(&named) == 0, "no crosshair");
    let image = look("image");
    assert!(red(&image) > 300, "no image cursor ({} red px)", red(&image));
    let hidden = look("hidden");
    assert!(white(&hidden) == 0 && red(&hidden) == 0, "the hidden cursor shows");
    let _vt = v.lock();
    let blocked = look("image");
    assert!(red(&blocked) == 0 && white(&blocked) > 10, "locked: not the not-allowed cursor");
}
