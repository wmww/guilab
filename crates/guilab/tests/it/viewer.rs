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
        c.call_as(&Request { proto: guilab_proto::PROTO, target: Target::default(), cmd: Command::Ping }).unwrap()
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
    if !ok && err.contains("software GLES failed") {
        eprintln!("skipping: no software EGL device: {err}");
        return;
    }
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

/// Sessions on pixman: the viewer draws with its own software GLES (llvmpipe).
#[test]
fn click_through_pixman() {
    click_through(&[("GUILAB_RENDERER", "pixman")], "shm");
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

/// Drawing and chat, end to end: the agent's message reaches the feed; the human draws a box
/// and a text note, types a message and sends it; `poll` lists the drawing (the frozen
/// screenshot plus both marks) and the image renders with the box in it.
/// Positions are the viewer's layout in a 1200x800 window (toolbar at the left, message
/// box at the bottom, the 800x600 session fitted in between).
#[test]
fn draw_and_send() {
    let Some((h, sock, t)) = host() else { return };
    let v = Lab::new();
    v.run(&["new", "app", "-s", "800x600"]);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    let shot = v.json(&["screenshot", "--no-settle"])["image"].as_str().unwrap().to_string();
    v.run(&["done", "here's the app", "--image", &shot]);

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
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    let id = lines.iter().find_map(|l| l.strip_prefix("image ")).and_then(|l| l.split(' ').next()).expect("an image");
    assert!(lines.iter().any(|l| l.starts_with("1 raster r") && l.contains("0,0 800x600 (screenshot")), "{out}");
    assert!(lines.iter().any(|l| l.starts_with("2 red box ")), "{out}");
    assert!(lines.iter().any(|l| l.starts_with("3 red text ") && l.contains("\"label\"")), "{out}");
    assert!(out.contains(&format!("(see what the marks point at: guilab image {id} --crop '")), "{out}");
    let img = v.json(&["image", id])["render"]["path"].as_str().unwrap().to_string();
    let (w, hh, px, ch) = crate::read_png(&img);
    assert_eq!((w, hh), (800, 600));
    let red = px.chunks(ch).filter(|p| p[0] > 180 && p[1] < 80 && p[2] < 80).count();
    assert!(red > 300, "the box isn't in the image ({red} red pixels)");
    assert!(v.info().viewer.is_some(), "the viewer survived");
    // `ls` says which tab the viewer shows; `-S NAME show` switches it.
    assert!(v.run(&["ls"]).contains("app 800x600 apps=0 windows=0 expires=30m (shown in the viewer)"));
    v.run(&["new", "other"]);
    let (ok, _, err) = v.try_run_env(&[("WAYLAND_DISPLAY", &sock)], &["-S", "other", "show"]);
    assert!(ok, "{err}");
    std::thread::sleep(Duration::from_millis(100));
    let ls = v.run(&["ls"]);
    assert!(ls.lines().any(|l| l.starts_with("other ") && l.ends_with("(shown in the viewer)")), "{ls}");
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

/// Scripted input without pauses: a click on the message box that comes with the pointer's
/// first move still focuses it, and keys typed right after the click go to it, not the app.
#[test]
fn click_then_type() {
    let Some((h, sock, t)) = host() else { return };
    h.run(&["-t", &t, "unlock"]);
    let v = Lab::new();
    v.run(&["new", "app", "-s", "800x600"]);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    h.run(&["debug", "input", "move", "600", "777", "click", "type", "hello", "key", "Return"]);
    let (_, out, err) = v.try_run(&["poll", "--timeout", "10"]);
    assert!(out.contains("human message: \"hello\""), "{out}{err}");
}

impl Lab {
    /// Waits until the session's output size satisfies `pred`; returns it.
    fn wait_output(&self, secs: u64, pred: impl Fn(u64, u64) -> bool) -> (u64, u64) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let o = self.json(&["output"]);
            let (w, h) = (o["width"].as_u64().unwrap(), o["height"].as_u64().unwrap());
            if pred(w, h) {
                return (w, h);
            }
            assert!(Instant::now() < deadline, "output stayed {w}x{h}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// View menu sizing: "Resize to window" sets the session's logical size to the view;
/// "Lock size to window" keeps it there as the window resizes, except while an agent holds
/// the lock (the agent may still resize); unlocking fits it again.
/// Layout (1200x800 host window, one session): View menu at 111,12, its "Resize to
/// window" row at y=127, "Lock size to window" at y=153; the view is the window minus the
/// 38 px toolbar, 24 px menu bar and 180 px chat panel.
#[test]
fn size_to_window() {
    let Some((h, sock, t)) = host() else { return };
    let v = Lab::new();
    v.run(&["new", "app", "-s", "800x600", "--scale", "1.5"]);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    let step = |actions: &[&str]| {
        h.input(&t, actions);
        std::thread::sleep(Duration::from_millis(150));
    };
    step(&["move", "111", "12", "click"]);
    step(&["move", "150", "127", "click"]);
    // Logical size = the view (1162x596 points), at scale 1.5.
    v.wait_output(5, |w, hh| (w, hh) == (1743, 894));
    let log = v.run(&["log", "--kind", "output"]);
    assert!(log.contains("output is now 1743x894 scale 1.5 (the human resized it)"), "{log}");

    step(&["move", "111", "12", "click"]);
    step(&["move", "150", "153", "click"]);
    h.run(&["-t", &t, "output", "-s", "1000x700"]);
    v.wait_output(5, |w, hh| (w, hh) == (1443, 744));
    // An agent's lock freezes it; the agent can still resize.
    let vt = v.lock();
    h.run(&["-t", &t, "output", "-s", "1100x750"]);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(v.wait_output(1, |_, _| true), (1443, 744));
    v.run(&["-t", &vt, "output", "-s", "640x480"]);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(v.wait_output(1, |_, _| true), (640, 480));
    v.run(&["-t", &vt, "unlock"]);
    v.wait_output(5, |w, hh| (w, hh) == (1593, 819));
}

/// Dragging the bottom-right resize handle resizes the output by the dragged distance.
#[test]
fn resize_handle() {
    let Some((h, sock, t)) = host() else { return };
    let v = Lab::new();
    v.run(&["new", "app", "-s", "600x400"]);
    v.run(&["run", "--", "sleep", "600"]);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    let m = mapping(&h, &t, &v);
    assert!((m.scale - 1.0).abs() < 0.01, "a 600x400 session shows 1:1 ({})", m.scale);
    let (x, y) = ((m.origin.0 + 600.0 + 5.0).round() as i64, (m.origin.1 + 400.0 + 5.0).round() as i64);
    let s = |v: i64| v.to_string();
    h.input(&t, &["move", &s(x), &s(y), "sleep", "100", "down", "move", &s(x + 100), &s(y + 50), "300", "sleep", "100", "up"]);
    v.wait_output(5, |w, hh| (w, hh) == (700, 450));
    // The human's moves never reached the app.
    let log = v.run(&["log", "--kind", "button"]);
    assert!(!log.contains("pressed"), "{log}");
}

/// At matching fractional scales (host and session 1.5) a session that fits shows pixel
/// for pixel: the viewer adds no blur.
#[test]
fn pixel_exact_at_fractional_scale() {
    let h = Lab::new();
    let info = h.json(&["new", "host", "-s", "1500x900", "--scale", "1.5"]);
    if !h.info().renderer.starts_with("gles") || !gtk4() {
        return;
    }
    let sock = info["wayland_display"].as_str().unwrap().to_string();
    // Sized at creation: an `output` change would put a note over the image (overlay).
    let v = Lab::new();
    v.run(&["new", "app", "-s", "750x450", "--scale", "1.5"]);
    // Labels only: no blinking caret to differ between the two screenshots.
    let log = v.json(&["run", "--", "python3", &app("gtk_cursors.py")])["log"].as_str().unwrap().to_string();
    wait_log(&log, "ready", 20);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    let (vs, hs) = (v.path("v.png"), h.path("h.png"));
    std::thread::sleep(Duration::from_millis(500));
    v.run(&["screenshot", "-o", &vs]);
    std::thread::sleep(Duration::from_millis(300));
    h.run(&["screenshot", "-o", &hs]);
    let (vw, vh, vp, vc) = crate::read_png(&vs);
    let (hw, hh, hp, hc) = crate::read_png(&hs);
    let (vw, vh, hw, hh) = (vw as usize, vh as usize, hw as usize, hh as usize);
    fn at(p: &[u8], c: usize, w: usize, x: usize, y: usize) -> &[u8] {
        &p[(y * w + x) * c..][..3]
    }
    let found = (0..=hh - vh).flat_map(|y| (0..=hw - vw).map(move |x| (x, y))).find(|&(ox, oy)| {
        (0..vh).step_by(7).all(|y| (0..vw).step_by(5).all(|x| at(&vp, vc, vw, x, y) == at(&hp, hc, hw, ox + x, oy + y)))
    });
    let (ox, oy) = found.expect("the session isn't shown pixel for pixel");
    let differ = (0..vh).flat_map(|y| (0..vw).map(move |x| (x, y))).filter(|&(x, y)| at(&vp, vc, vw, x, y) != at(&hp, hc, hw, ox + x, oy + y)).count();
    assert_eq!(differ, 0, "{differ} pixels differ at offset {ox},{oy}");
}

/// Host touch: a tap on the app's image reaches the app as touch; a tap on the viewer's
/// own UI acts like a click there (here: the box tool, then a touch drag draws a box).
#[test]
fn touch_through() {
    let Some((h, sock, t)) = host() else { return };
    if !gtk4() {
        return;
    }
    let (v, log) = viewed(&[]);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    let m = mapping(&h, &t, &v);
    let (bx, by) = center(&std::fs::read_to_string(&log).unwrap(), "button", 1.0);
    let x = (m.origin.0 + bx.parse::<f64>().unwrap() * m.scale).round() as i64;
    let y = (m.origin.1 + by.parse::<f64>().unwrap() * m.scale).round() as i64;
    h.input(&t, &["tap", &x.to_string(), &y.to_string()]);
    wait_log(&log, "clicked 1", 5);
    let fingers = v.run(&["log", "--kind", "touch"]);
    assert!(fingers.contains("finger 0 down"), "{fingers}");
    // Toolbar: box tool at 19,133 (one session, no tab bar).
    h.input(&t, &["tap", "19", "133", "sleep", "150", "touch", "down", "1", &(x - 100).to_string(), &(y + 150).to_string(), "touch", "move", "1", &(x + 50).to_string(), &(y + 250).to_string(), "300", "touch", "up", "1"]);
    std::thread::sleep(Duration::from_millis(300));
    let shot = h.path("touch.png");
    h.run(&["screenshot", "-o", &shot]);
    let (w, _, px, ch) = crate::read_png(&shot);
    let red = (y + 150..y + 250).filter(|yy| {
        let i = ((yy * w as i64 + x - 100) as usize) * ch;
        px[i] > 180 && px[i + 1] < 80 && px[i + 2] < 80
    }).count();
    assert!(red > 50, "no box drawn along its left edge ({red} red px)");
}

/// The host clipboard and the shown session's are bridged (text): host → session always,
/// session → host while the viewer has focus.
#[test]
fn clipboard_bridge() {
    let Some((h, sock, t)) = host() else { return };
    if !gtk4() {
        return;
    }
    let (v, log) = viewed(&[]);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    let m = mapping(&h, &t, &v);
    let (ex, ey) = center(&std::fs::read_to_string(&log).unwrap(), "entry", 1.0);
    let x = (m.origin.0 + ex.parse::<f64>().unwrap() * m.scale).round() as i64;
    let y = (m.origin.1 + ey.parse::<f64>().unwrap() * m.scale).round() as i64;
    h.input(&t, &["move", &x.to_string(), &y.to_string(), "click"]);
    h.run(&["-t", &t, "clip", "set", "from host"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while v.try_run(&["clip", "get"]).1.trim() != "from host" {
        assert!(Instant::now() < deadline, "host text never reached the session");
        std::thread::sleep(Duration::from_millis(50));
    }
    h.input(&t, &["key", "ctrl+v"]);
    wait_log(&log, "entry 'from host'", 5); // pasting is asynchronous
    h.input(&t, &["type", " + app", "key", "ctrl+a", "key", "ctrl+c"]);
    wait_log(&log, "entry 'from host + app'", 5);
    while h.try_run(&["clip", "get"]).1.trim() != "from host + app" {
        assert!(Instant::now() < deadline, "the app's copy never reached the host: {}", h.try_run(&["clip", "get"]).1);
        std::thread::sleep(Duration::from_millis(50));
    }
}
