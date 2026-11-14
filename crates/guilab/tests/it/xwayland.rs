//! Xwayland (`new -x`): X windows in `tree`, input, menus, dialogs, clipboard bridging,
//! parallel sessions and cleanup. Skipped without Xwayland or Python's tkinter (the X test
//! app is `apps/tk_app.py`).

use std::{path::Path, time::Duration};

use serde_json::Value;

use crate::{
    Lab, app,
    cleanup::{alive, pgrep},
    have,
    input::{center, gtk4, has_python_module, wait_log},
    nodes, rect,
};

fn xwayland() -> bool {
    have("Xwayland") && has_python_module("import tkinter")
}

fn kind<'a>(tree: &'a Value, k: &str) -> Vec<&'a Value> {
    nodes(tree).into_iter().filter(|n| n["kind"] == k).collect()
}

/// Starts the Tk app; returns its log once it's laid out.
fn tk(lab: &Lab, session: &str) -> (String, String) {
    let log = lab.json(&["-S", session, "run", "--", "python3", &app("tk_app.py")])["log"].as_str().unwrap().to_string();
    let l = wait_log(&log, "ready", 15);
    (log, l)
}

#[test]
fn x11_windows_input_menus_dialogs() {
    if !xwayland() {
        return;
    }
    let lab = Lab::new();
    let s = lab.json(&["new", "-x"]);
    assert!(s["x_display"].as_str().is_some_and(|d| d.starts_with(':')), "{s}");
    let (log, l) = tk(&lab, "s1");
    let tree = lab.wait_tree("s1", 5, |t| !kind(t, "x11").is_empty());
    let w = kind(&tree, "x11")[0];
    assert_eq!(w["class"], "Guilabtk");
    assert_eq!(w["title"], "guilab tk test");
    assert!(w["xid"].as_u64().is_some());
    assert_eq!(rect(w), (0, 0, 1280, 720), "the first window fills the output");
    let shot = lab.json(&["screenshot"]);
    assert!(shot["settled"].as_bool().unwrap());
    assert!(crate::lit_fraction(shot["path"].as_str().unwrap()) > 0.5);

    // Typing, including characters the keymap lacks (or has only above X's code 255: €).
    let t = lab.lock();
    let c = |n: &str| center(&l, n, 1.0);
    let (ex, ey) = c("entry");
    lab.input(&t, &["move", &ex, &ey, "click", "type", "Hi é€ 日本 ok"]);
    wait_log(&log, "edit 'Hi é€ 日本 ok'", 5);
    // More new characters than fit in one keymap: several swaps.
    let many = "ĀāĂăĄąĆćĈĉĊċČčĎďĐđĒēĔĕĖėĘęĚěĜĝĞğĠġĢģĤĥĦħĨĩĪīĬĭĮįİı!";
    lab.input(&t, &["type", many]);
    wait_log(&log, &format!("edit 'Hi é€ 日本 ok{many}'"), 5);

    // A menu (override-redirect window) under its button; pick an item with `on`.
    let (mx, my) = c("menu");
    lab.input(&t, &["move", &mx, &my, "click"]);
    let tree = lab.wait_tree("s1", 5, |t| !kind(t, "x11_popup").is_empty());
    assert_eq!(tree[0]["kind"], "x11_popup", "menus are on top: {tree:#}");
    let m = kind(&tree, "x11_popup")[0];
    let (_, my0, _, mh) = rect(m);
    assert!((my0 - my.parse::<i64>().unwrap()).abs() < 40, "menu at its button: {m}");
    lab.input(&t, &["on", &m["id"].to_string(), "move", "20", &format!("{}", mh * 3 / 10), "click"]);
    wait_log(&log, "menu 2", 5);
    lab.wait_tree("s1", 5, |t| kind(t, "x11_popup").is_empty());

    // A transient dialog floats centered, above its parent even after clicking the parent.
    let (dx, dy) = c("dialog");
    lab.input(&t, &["move", &dx, &dy, "click"]);
    let tree = lab.wait_tree("s1", 5, |t| kind(t, "x11").len() == 2);
    assert_eq!(tree[0]["title"], "guilab tk dialog");
    let (x, y, w, h) = rect(&tree[0]);
    assert_eq!((x + w / 2, y + h / 2), (640, 360), "{tree:#}");
    lab.input(&t, &["move", "20", "700", "click"]);
    assert_eq!(lab.json(&["tree"])[0]["title"], "guilab tk dialog");
}

#[test]
fn x11_clipboard_bridging() {
    if !xwayland() {
        return;
    }
    let lab = Lab::new();
    lab.run(&["new", "-x"]);
    let (log, l) = tk(&lab, "s1");
    let t = lab.lock();
    let (ex, ey) = center(&l, "entry", 1.0);

    // Ours -> X, X -> ours. (Tk: ctrl+/ selects all.)
    lab.run(&["-t", &t, "clip", "set", "from guilab ü"]);
    lab.input(&t, &["move", &ex, &ey, "click", "key", "ctrl+v"]);
    wait_log(&log, "edit 'from guilab ü'", 5);
    lab.input(&t, &["type", " + tk", "key", "ctrl+slash", "key", "ctrl+c"]);
    lab.wait_clip("from guilab ü + tk", 5);

    // X -> Wayland app -> X.
    if !gtk4() {
        return;
    }
    let glog = lab.json(&["-t", &t, "run", "--", &app("gtk_input.py")])["log"].as_str().unwrap().to_string();
    let gl = wait_log(&glog, "ready", 15);
    // It floats centered over the tiled Tk window.
    let tree = lab.wait_tree("s1", 5, |t| !kind(t, "toplevel").is_empty());
    let g = kind(&tree, "toplevel")[0];
    let (gx, gy, _, _) = rect(g);
    let (cx, cy) = center(&gl, "entry", 1.0);
    let (gex, gey) = (format!("{}", gx + cx.parse::<i64>().unwrap()), format!("{}", gy + cy.parse::<i64>().unwrap()));
    lab.input(&t, &["move", &gex, &gey, "click", "key", "ctrl+v"]);
    wait_log(&glog, "entry 'from guilab ü + tk'", 5);
    lab.input(&t, &["key", "ctrl+a", "type", "from gtk", "key", "ctrl+a", "key", "ctrl+c"]);
    lab.wait_clip("from gtk", 5);
    lab.input(&t, &["move", &ex, &ey, "click", "key", "ctrl+slash", "key", "BackSpace", "key", "ctrl+v"]);
    wait_log(&log, "edit 'from gtk'", 5);
}

/// The X socket and lock file of display `:N`.
fn x_files(display: &str) -> [String; 2] {
    let n = display.trim_start_matches(':');
    [format!("/tmp/.X11-unix/X{n}"), format!("/tmp/.X{n}-lock")]
}

#[test]
fn x11_parallel_sessions_and_cleanup() {
    if !xwayland() {
        return;
    }
    let lab = Lab::new();
    let a = lab.json(&["new", "a", "-x"])["x_display"].as_str().unwrap().to_string();
    let b = lab.json(&["new", "b", "-x"])["x_display"].as_str().unwrap().to_string();
    assert_ne!(a, b);
    let (_, la) = tk(&lab, "a");
    tk(&lab, "b");
    for s in ["a", "b"] {
        lab.wait_tree(s, 5, |t| kind(t, "x11").len() == 1);
    }
    let xpid = |d: &str| pgrep(&format!("Xwayland {d} "));
    let (xa, xb) = (xpid(&a), xpid(&b));
    assert!(xa.len() == 1 && xb.len() == 1);
    let app_a = lab.json(&["-S", "a", "ps"])[0]["pid"].as_u64().unwrap();
    assert!(x_files(&a).iter().all(|f| Path::new(f).exists()));

    // Input reaches only its session's app.
    lab.run(&["-S", "a", "work", "testing input"]);
    let t = lab.json(&["-S", "a", "lock"])["token"].as_str().unwrap().to_string();
    let (ex, ey) = center(&la, "entry", 1.0);
    lab.run(&["-S", "a", "-t", &t, "input", "move", &ex, &ey, "click", "type", "only a"]);
    let logs: Vec<String> = ["a", "b"].iter().map(|s| lab.json(&["-S", s, "ps"])[0]["log"].as_str().unwrap().to_string()).collect();
    wait_log(&logs[0], "edit 'only a'", 5);
    assert!(!std::fs::read_to_string(&logs[1]).unwrap().contains("only a"));

    // Ending a session takes its Xwayland, X clients, socket and lock file with it.
    lab.run(&["-S", "a", "-t", &t, "end"]);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while (alive(xa[0]) || alive(app_a)) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!alive(xa[0]) && !alive(app_a));
    assert!(x_files(&a).iter().all(|f| !Path::new(f).exists()));
    assert!(alive(xb[0]));
    lab.run(&["-S", "b", "end"]);
}

/// HiDPI: X clients render at the output scale (X pixels = output pixels), so input and
/// layout work the same as at scale 1; windows without `_NET_WM_PID` get a pid from
/// X-Resource.
#[test]
fn x11_hidpi_and_pids() {
    if !xwayland() {
        return;
    }
    let lab = Lab::new();
    lab.run(&["new", "-x", "-s", "1600x1000", "--scale", "2"]);
    let (log, l) = tk(&lab, "s1");
    let tree = lab.wait_tree("s1", 5, |t| !kind(t, "x11").is_empty());
    assert_eq!(rect(kind(&tree, "x11")[0]), (0, 0, 1600, 1000));
    // Tk reports X pixels: the entry spans the 1600 px wide window.
    let entry: Vec<i64> = l.lines().find(|x| x.starts_with("pos entry ")).unwrap().split(' ').skip(2).map(|v| v.parse().unwrap()).collect();
    assert!(entry[2] > 1500, "the X window isn't at the output's pixel size: {entry:?}");
    let t = lab.lock();
    let (ex, ey) = center(&l, "entry", 1.0);
    lab.input(&t, &["move", &ex, &ey, "click", "type", "hi"]);
    wait_log(&log, "edit 'hi'", 5);
    if crate::have("xeyes") {
        lab.run(&["-t", &t, "run", "--", "xeyes"]);
        let tree = lab.wait_tree("s1", 10, |t| kind(t, "x11").iter().any(|n| n["class"] == "XEyes"));
        let eyes = kind(&tree, "x11").into_iter().find(|n| n["class"] == "XEyes").unwrap().clone();
        assert!(eyes["pid"].as_u64().is_some(), "no pid for xeyes: {eyes}");
    }
}

/// Drag and drop from an X app (GTK4 on X11) to a Wayland app (GTK4).
#[test]
fn x11_to_wayland_dnd() {
    if !xwayland() || !gtk4() {
        return;
    }
    let lab = Lab::new();
    lab.run(&["new", "-x", "-s", "1600x1000"]);
    let wl = lab.json(&["run", "--", "python3", &app("gtk_input.py")])["log"].as_str().unwrap().to_string();
    let wll = wait_log(&wl, "ready", 15);
    let x = lab.json(&["run", "--", "env", "GDK_BACKEND=x11", "python3", &app("gtk_input.py")])["log"].as_str().unwrap().to_string();
    let xl = wait_log(&x, "ready", 15);
    // The X window floats over the tiled Wayland one; its drag source, the Wayland drop
    // target (bottom right) stays visible.
    let tree = lab.wait_tree("s1", 5, |t| !kind(t, "x11").is_empty());
    let (wx, wy, _, _) = rect(kind(&tree, "x11")[0]);
    let (sx, sy) = center(&xl, "drag", 1.0);
    let (sx, sy) = (wx + sx.parse::<i64>().unwrap(), wy + sy.parse::<i64>().unwrap());
    let (dx, dy) = center(&wll, "drop", 1.0);
    let s = |v: i64| v.to_string();
    let t = lab.lock();
    lab.input(&t, &["move", &s(sx), &s(sy), "down", "move", &s(sx + 15), &s(sy + 10), "100", "move", &dx, &dy, "400", "sleep", "300", "up"]);
    wait_log(&wl, "dropped payload", 5);
}
