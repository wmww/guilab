//! Real apps: rendering, window policy, popups, output changes, parallel sessions.

use crate::{Lab, app, have, lit_fraction, nodes, read_png, rect};

#[test]
fn shm_client_renders() {
    if !have("weston-simple-shm") {
        return;
    }
    let lab = Lab::new();
    lab.run(&["new", "-s", "400x300"]);
    lab.run(&["run", "--", "weston-simple-shm"]);
    let shot = lab.path("shm.png");
    // simple-shm animates, so it never settles; it must still show up.
    let s = lab.json(&["screenshot", "-o", &shot]);
    assert_eq!((s["width"].as_u64(), s["height"].as_u64()), (Some(400), Some(300)));
    assert!(lit_fraction(&shot) > 0.2, "screenshot is mostly black");
    let t = lab.json(&["tree"]);
    assert_eq!(t[0]["kind"], "toplevel");
    assert_eq!(t[0]["app_id"], "org.freedesktop.weston.simple-shm");
}

#[test]
fn gtk_policy_popup_dialog() {
    if !have("python3") || std::process::Command::new("python3").args(["-c", "import gi; gi.require_version('Gtk','4.0')"]).status().map(|s| !s.success()).unwrap_or(true) {
        eprintln!("skipping: python3 GTK4 bindings missing");
        return;
    }
    let lab = Lab::new();
    lab.run(&["new", "-s", "640x480"]);
    lab.run(&["run", "--", &app("gtk_app.py"), "--menu", "--dialog"]);
    let t = lab.wait_tree("s1", 15, |t| {
        nodes(t).iter().any(|n| n["kind"] == "popup") && t.as_array().unwrap().len() == 2
    });
    // The dialog floats centered over the tiled main window.
    let dialog = t.as_array().unwrap().iter().find(|n| n["title"] == "guilab dialog").unwrap();
    let main = t.as_array().unwrap().iter().find(|n| n["title"] == "guilab test").unwrap_or_else(|| panic!("{t:#}"));
    assert_eq!(rect(main), (0, 0, 640, 480));
    let (x, y, w, h) = rect(dialog);
    assert!((x + w / 2 - 320).abs() <= 1 && (y + h / 2 - 240).abs() <= 1, "dialog not centered: {dialog}");
    // The menu button is in the bottom-right corner; the popup must be moved on screen.
    let popup = nodes(&main["children"]).into_iter().find(|n| n["kind"] == "popup").unwrap();
    let (px, py, pw, ph) = rect(popup);
    assert!(px >= 0 && py >= 0 && px + pw <= 640 && py + ph <= 480, "popup off screen: {popup}");

    let shot = lab.path("gtk.png");
    let s = lab.json(&["screenshot", "-o", &shot]);
    assert_eq!(s["settled"], true);
    assert!(lit_fraction(&shot) > 0.5);
}

#[test]
fn output_resize_and_scale() {
    if !have("gtk4-demo") {
        return;
    }
    let lab = Lab::new();
    lab.run(&["new", "-s", "800x600"]);
    lab.run(&["run", "--", "gtk4-demo"]);
    lab.wait_tree("s1", 15, |t| t.as_array().is_some_and(|a| !a.is_empty()));
    lab.run(&["output", "-s", "1000x500", "--scale", "1.5"]);
    let shot = lab.path("scaled.png");
    let s = lab.json(&["screenshot", "-o", &shot]);
    assert_eq!(s["settled"], true);
    let (w, h, _, _) = read_png(&shot);
    assert_eq!((w, h), (1000, 500));
    // 500px at scale 1.5 is 333.3 logical; the window gets 334, so allow a pixel.
    let (x, y, w, h) = rect(&lab.json(&["tree"])[0]);
    assert!(x == 0 && y == 0 && w == 1000 && (500..=501).contains(&h), "{x},{y} {w}x{h}");
    // Screenshots right after a change must show the new layout: a later one is identical.
    std::thread::sleep(std::time::Duration::from_millis(700));
    let shot2 = lab.path("scaled2.png");
    lab.json(&["screenshot", "-o", &shot2]);
    assert!(read_png(&shot).2 == read_png(&shot2).2, "screenshot taken before the app finished resizing");
}

#[test]
fn parallel_sessions_are_isolated() {
    if !have("weston-simple-shm") || !have("kitty") {
        return;
    }
    let lab = Lab::new();
    lab.run(&["new", "a", "-s", "320x240"]);
    lab.run(&["new", "b", "-s", "300x200"]);
    lab.run(&["-S", "a", "run", "--", "weston-simple-shm"]);
    lab.run(&["-S", "b", "run", "--", "kitty", "-o", "confirm_os_window_close=0"]);
    let ta = lab.wait_tree("a", 15, |t| t.as_array().is_some_and(|a| a.len() == 1));
    let tb = lab.wait_tree("b", 15, |t| t.as_array().is_some_and(|a| a.len() == 1));
    assert_eq!(ta[0]["app_id"], "org.freedesktop.weston.simple-shm");
    assert_eq!(tb[0]["app_id"], "kitty");
    assert_eq!(rect(&tb[0]), (0, 0, 300, 200));
    let (sa, sb) = (lab.path("a.png"), lab.path("b.png"));
    lab.json(&["-S", "a", "screenshot", "-o", &sa]);
    lab.json(&["-S", "b", "screenshot", "-o", &sb]);
    assert_eq!(read_png(&sa).0, 320);
    assert_eq!(read_png(&sb).0, 300);
}
