//! The viewer window, shown on a headless host: a second guilab server's session plays the
//! human's desktop. The test drives the host session's pointer (as an agent there), which
//! the viewer turns into human input in the viewed session.

use crate::*;

impl Lab {
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
        wait_until("human motion in the viewed session", 10, || {
            h.input(t, &["move", &x.to_string(), &y.to_string()]);
            // `-n` doesn't count motion: this lists every motion event.
            let log = v.json(&["log", "--kind", "motion", "-n", "1"]);
            let last = log["events"].as_array().and_then(|e| e.last()).and_then(|e| e["path"].as_array()).and_then(|p| p.last()).cloned();
            match last.map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap())) {
                Some(at) if Some(at) != prev => Ok(at),
                _ => Err(format!("{log}\n{}", v.run(&["log"]))),
            }
        })
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

    // Locked by an agent: the human's clicks are dropped, until it unlocks. (The pinch
    // after the last click shows the app handled everything before it.)
    let vt = v.lock();
    click_button(&h, &t, &m, &log);
    wait_until("the viewer to drop the click", 10, || {
        let d = v.info().viewer.map_or(0, |i| i.dropped);
        if d > 0 { Ok(()) } else { Err(format!("{d} dropped")) }
    });
    v.run(&["-t", &vt, "unlock"]);
    click_button(&h, &t, &m, &log);
    h.input(&t, &["move", &x.to_string(), &y.to_string(), "sleep", "100", "pinch", "2"]);
    wait_until("the second pinch", 10, || {
        let l = std::fs::read_to_string(&log).unwrap();
        if l.matches("zoom end").count() == 2 { Ok(()) } else { Err(l) }
    });
    assert_eq!(clicks(&log), 2, "a click got through the lock: {}", v.run(&["log"]));
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
    wait_until("the viewer to go when the host died", 10, || v.info().viewer.map_or(Ok(()), |i| Err(format!("{i:?}"))));
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
/// and a text note on the live app (a draft, kept while the viewer is hidden), types a
/// message and sends it; `poll` lists the drawing (the screenshot plus both marks) and the
/// image renders with the box in it. Then the agent sends an image; the human opens it from
/// the feed (a message sent meanwhile names it as open), draws on it (a copy) and sends that.
/// Positions are the viewer's layout in a 1200x800 window: tools at x=19 (box y=140, arrow
/// y=236, text y=268), the message box at y=768, the newest feed image at 100,645 (above
/// its one-line text and the "done" line), the session fitted in between.
#[test]
fn draw_and_send() {
    let Some((h, sock, t)) = host() else { return };
    let v = Lab::new();
    v.run(&["new", "app", "-s", "800x600"]);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    let shot = v.json(&["screenshot", "--no-settle"])["image"].as_str().unwrap().to_string();
    v.run(&["done", "here's the app", "--image", &shot]);

    // No pauses needed: the viewer runs a UI pass per click (see `HostState::take_input`).
    let step = |actions: &[&str]| {
        h.input(&t, actions);
    };
    step(&["move", "19", "140", "click"]); // box tool
    step(&["move", "500", "300", "down", "move", "700", "400", "100", "up"]);
    step(&["move", "19", "268", "click"]); // text tool
    step(&["move", "520", "330", "click"]);
    step(&["type", "label", "key", "Return"]);
    // Hiding the viewer keeps the draft.
    v.wait_drafts(&[3]);
    v.run(&["hide"]);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    step(&["move", "600", "768", "click"]); // message box
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
    let (w, hh, px, ch) = read_png(&img);
    assert_eq!((w, hh), (800, 600));
    let red = px.chunks(ch).filter(|p| p[0] > 180 && p[1] < 80 && p[2] < 80).count();
    assert!(red > 300, "the box isn't in the image ({red} red pixels)");

    // The agent sends an image (white, so the test sees it arrive in the feed).
    let made = v.json(&["image", "make", "--canvas", "400x300", "--color", "white", "red box 50,50 100x80"])["id"].as_str().unwrap().to_string();
    v.run(&["done", "like this?", "--image", &made]);
    let host_shot = h.path("feed.png");
    wait_until("the agent's image in the feed", 10, || {
        h.run(&["screenshot", "--no-settle", "-o", &host_shot]);
        let (w, _, px, ch) = read_png(&host_shot);
        let p = &px[(645 * w as usize + 100) * ch..][..3];
        if p.iter().all(|c| *c > 230) { Ok(()) } else { Err(format!("{p:?}")) }
    });
    // Open it; a message sent now names it (not attached).
    step(&["move", "100", "645", "click"]);
    step(&["move", "600", "768", "click"]);
    step(&["type", "this one?", "key", "Return"]);
    let (_, out, err) = v.try_run(&["poll", "--timeout", "10"]);
    assert!(out.contains(&format!("(they had image {made} open in their editor, not attached")), "{out}{err}");
    v.run(&["done", "yes"]);
    // Drawing on it makes a copy, which goes with the next message. The 400x300 image is
    // shown 1:1 at 420,142.
    step(&["move", "19", "236", "click"]); // arrow tool
    step(&["move", "500", "200", "down", "move", "600", "300", "100", "up"]);
    step(&["move", "600", "768", "click"]);
    step(&["type", "and here", "key", "Return"]);
    let (_, out, err) = v.try_run(&["poll", "--timeout", "10"]);
    assert!(out.contains(&format!(", parent {made}")), "{out}{err}");
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    assert!(lines.iter().any(|l| l.starts_with("1 red box 50,50 100x80")), "{out}");
    assert!(lines.iter().any(|l| l.starts_with("2 red arrow 80,58 to 180,158")), "{out}");
    assert!(v.run(&["image", &made, "--list"]).lines().count() == 2, "the agent's image is unchanged");

    assert!(v.info().viewer.is_some(), "the viewer survived");
    // `ls` says which tab the viewer shows; `-S NAME show` switches it.
    assert!(v.run(&["ls"]).contains("app 800x600 apps=0 windows=0 expires=30m (shown in the viewer)"));
    v.run(&["new", "other"]);
    let (ok, _, err) = v.try_run_env(&[("WAYLAND_DISPLAY", &sock)], &["-S", "other", "show"]);
    assert!(ok, "{err}");
    wait_until("the viewer to show `other`", 10, || {
        let ls = v.run(&["ls"]);
        if ls.lines().any(|l| l.starts_with("other ") && l.ends_with("(shown in the viewer)")) { Ok(()) } else { Err(ls) }
    });
}

/// File > Insert image (`debug insert` picks the file): on the live app it makes a draft on
/// a screenshot with the picture in it; a guilab rendering is refused.
#[test]
fn insert_image() {
    let Some((h, sock, t)) = host() else { return };
    let v = Lab::new();
    v.run(&["new", "app", "-s", "800x600"]);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    let pic = v.path("pic.png");
    write_png(&pic, 40, 20);
    // The rendering goes first, so once the picture is in, the rendering was handled.
    let rendering = v.json(&["screenshot", "--no-settle"])["path"].as_str().unwrap().to_string();
    v.run(&["debug", "insert", &rendering]);
    v.run(&["debug", "insert", &pic]);
    v.wait_drafts(&[2]);
    h.input(&t, &["move", "600", "768", "click", "type", "with a picture", "key", "Return"]);
    let (_, out, err) = v.try_run(&["poll", "--timeout", "10"]);
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    assert!(lines.iter().any(|l| l.starts_with("1 raster r") && l.contains("(screenshot")), "{out}{err}");
    assert!(lines.iter().any(|l| l.starts_with("2 raster r") && l.contains("380,290 40x20 (from pic.png)")), "{out}");
    assert!(!lines.iter().any(|l| l.starts_with("3 ")), "the rendering was refused: {out}");
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
    h.run(&["debug", "input", "move", "600", "768", "sleep", "100", "click"]); // message box
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
    let white = |p: &[[u8; 3]]| p.iter().filter(|c| c.iter().all(|v| *v > 220)).count();
    let red = |p: &[[u8; 3]]| p.iter().filter(|c| c[0] > 200 && c[1] < 50 && c[2] < 50).count();
    // Moves the pointer over `area` (60 px below the label's center, off its text) and
    // waits until the host's pixels around the pointer pass `ok`.
    let look = |area: &str, what: &str, ok: &dyn Fn(&[[u8; 3]]) -> bool| {
        let (x, y) = center(&applog, area, 1.0);
        let x = (m.origin.0 + x.parse::<f64>().unwrap() * m.scale).round() as i64;
        let y = (m.origin.1 + (y.parse::<f64>().unwrap() + 60.0) * m.scale).round() as i64;
        h.input(&t, &["move", &(x - 5).to_string(), &y.to_string(), "move", &x.to_string(), &y.to_string()]);
        wait_until(what, 10, || {
            h.run(&["screenshot", "--cursor", "-o", &shot]);
            let (w, _, px, ch) = read_png(&shot);
            let mut out = Vec::new();
            for dy in -12..=12 {
                for dx in -12..=12 {
                    let i = (((y + dy) * w as i64 + x + dx) as usize) * ch;
                    out.push([px[i], px[i + 1], px[i + 2]]);
                }
            }
            if ok(&out) { Ok(()) } else { Err(format!("{} white, {} red px", white(&out), red(&out))) }
        });
    };
    look("named", "a crosshair", &|p| white(p) > 10 && red(p) == 0);
    look("image", "the image cursor", &|p| red(p) > 300);
    look("hidden", "no cursor", &|p| white(p) == 0 && red(p) == 0);
    let _vt = v.lock();
    look("image", "locked: the not-allowed cursor", &|p| red(p) == 0 && white(p) > 10);
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
    h.run(&["debug", "input", "move", "600", "768", "click", "type", "hello", "key", "Return"]);
    let (_, out, err) = v.try_run(&["poll", "--timeout", "10"]);
    assert!(out.contains("human message: \"hello\""), "{out}{err}");
}

impl Lab {
    /// Waits until the session's output size satisfies `pred`; returns it.
    #[track_caller]
    fn wait_output(&self, secs: u64, pred: impl Fn(u64, u64) -> bool) -> (u64, u64) {
        wait_until("an output size", secs, || {
            let o = self.json(&["output"]);
            let (w, h) = (o["width"].as_u64().unwrap(), o["height"].as_u64().unwrap());
            if pred(w, h) { Ok((w, h)) } else { Err(format!("{w}x{h}")) }
        })
    }

    /// Waits until the viewer's drafts have these element counts.
    #[track_caller]
    fn wait_drafts(&self, want: &[usize]) {
        wait_until(&format!("drafts {want:?}"), 10, || {
            let d = self.info().viewer.map(|v| v.drafts);
            if d.as_deref() == Some(want) { Ok(()) } else { Err(format!("{d:?}")) }
        });
    }

    /// Waits until the viewer's host window has this size.
    #[track_caller]
    fn wait_window(&self, size: [u32; 2]) {
        wait_until(&format!("a {size:?} viewer window"), 10, || {
            let w = self.info().viewer.map(|v| v.window);
            if w == Some(size) { Ok(()) } else { Err(format!("{w:?}")) }
        });
    }
}

/// View menu sizing: "Resize to window" sets the session's logical size to the view;
/// "Lock size to window" keeps it there as the window resizes, except while an agent holds
/// the lock (the agent may still resize); unlocking fits it again.
/// Layout (1200x800 host window, one session): View menu at 111,12, its "Resize to
/// window" row at y=127, "Lock size to window" at y=153; the view is the window minus the
/// 40 px toolbar, 24 px menu bar and 240 px chat panel.
#[test]
fn size_to_window() {
    let Some((h, sock, t)) = host() else { return };
    let v = Lab::new();
    v.run(&["new", "app", "-s", "800x600", "--scale", "1.5"]);
    let (ok, _, err) = v.show(&sock);
    assert!(ok, "{err}");
    let step = |actions: &[&str]| {
        h.input(&t, actions);
    };
    step(&["move", "111", "12", "click"]);
    step(&["move", "150", "127", "click"]);
    // Logical size = the view (1160x536 points), at scale 1.5.
    v.wait_output(5, |w, hh| (w, hh) == (1740, 804));
    let log = v.run(&["log", "--kind", "output"]);
    assert!(log.contains("output is now 1740x804 scale 1.5 (the human resized it)"), "{log}");

    step(&["move", "111", "12", "click"]);
    step(&["move", "150", "153", "click"]);
    h.run(&["-t", &t, "output", "-s", "1000x700"]);
    v.wait_output(5, |w, hh| (w, hh) == (1440, 654));
    // An agent's lock freezes it; the agent can still resize.
    let vt = v.lock();
    h.run(&["-t", &t, "output", "-s", "1100x750"]);
    v.wait_window([1100, 750]);
    v.run(&["-t", &vt, "output", "-s", "640x480"]);
    assert_eq!(v.wait_output(1, |_, _| true), (640, 480));
    let log = v.run(&["log", "--kind", "output"]);
    assert_eq!(log.matches("the human resized it").count(), 2, "resized while locked: {log}");
    v.run(&["-t", &vt, "unlock"]);
    v.wait_output(5, |w, hh| (w, hh) == (1590, 729));
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
    fn at(p: &[u8], c: usize, w: usize, x: usize, y: usize) -> &[u8] {
        &p[(y * w + x) * c..][..3]
    }
    // Until the host shows the viewer's latest frame.
    wait_until("the session shown pixel for pixel", 10, || {
        v.run(&["screenshot", "-o", &vs]);
        h.run(&["screenshot", "-o", &hs]);
        let (vw, vh, vp, vc) = read_png(&vs);
        let (hw, hh, hp, hc) = read_png(&hs);
        let (vw, vh, hw, hh) = (vw as usize, vh as usize, hw as usize, hh as usize);
        let found = (0..=hh - vh).flat_map(|y| (0..=hw - vw).map(move |x| (x, y))).find(|&(ox, oy)| {
            (0..vh).step_by(7).all(|y| (0..vw).step_by(5).all(|x| at(&vp, vc, vw, x, y) == at(&hp, hc, hw, ox + x, oy + y)))
        });
        let (ox, oy) = found.ok_or("the session isn't in the host's screenshot")?;
        let differ = (0..vh).flat_map(|y| (0..vw).map(move |x| (x, y))).filter(|&(x, y)| at(&vp, vc, vw, x, y) != at(&hp, hc, hw, ox + x, oy + y)).count();
        if differ == 0 { Ok(()) } else { Err(format!("{differ} pixels differ at offset {ox},{oy}")) }
    });
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
    let shot = h.path("touch.png");
    wait_until("a box drawn along its left edge", 10, || {
        h.run(&["screenshot", "-o", &shot]);
        let (w, _, px, ch) = read_png(&shot);
        let red = (y + 150..y + 250)
            .filter(|yy| {
                let i = ((yy * w as i64 + x - 100) as usize) * ch;
                px[i] > 180 && px[i + 1] < 80 && px[i + 2] < 80
            })
            .count();
        if red > 50 { Ok(()) } else { Err(format!("{red} red px")) }
    });
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
    v.wait_clip("from host", 10);
    h.input(&t, &["key", "ctrl+v"]);
    wait_log(&log, "entry 'from host'", 5); // pasting is asynchronous
    h.input(&t, &["type", " + app", "key", "ctrl+a", "key", "ctrl+c"]);
    wait_log(&log, "entry 'from host + app'", 5);
    h.wait_clip("from host + app", 10);
}
