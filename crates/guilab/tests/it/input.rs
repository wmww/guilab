//! Locking, input chains and the clipboard, against real apps. The test apps print what
//! they receive to their log (see tests/it/apps/), which is how these tests check delivery.

use std::time::{Duration, Instant};

use crate::*;

#[test]
fn lock_rules() {
    let lab = Lab::new();
    lab.run(&["new"]);
    // Input and lock need the working state; input always needs the lock; other changes
    // only while locked.
    for cmd in [&["input", "move", "1", "1"][..], &["lock"]] {
        let (ok, _, err) = lab.try_run(cmd);
        assert!(!ok && err.contains("guilab work \"what you're doing\""), "{err}");
    }
    lab.run(&["work", "testing locks"]);
    let (ok, _, err) = lab.try_run(&["input", "move", "1", "1"]);
    assert!(!ok && err.contains("guilab lock"), "{err}");
    lab.run(&["run", "--", "true"]);
    let t = lab.lock();
    assert_eq!(lab.json(&["-t", &t, "lock"])["token"], t.as_str(), "relocking with the token keeps it");
    let (ok, _, err) = lab.try_run(&["lock"]);
    assert!(!ok && err.contains("already locked") && err.contains("--force"), "{err}");
    let (ok, _, err) = lab.try_run(&["run", "--", "true"]);
    assert!(!ok && err.contains("-t TOKEN"), "{err}");
    lab.run(&["-t", &t, "run", "--", "true"]);
    lab.run(&["ps"]); // read-only: no token needed
    assert_eq!(lab.input(&t, &["move", "10", "20"])["pointer"], serde_json::json!([10.0, 20.0]));
    // Bad chains fail before doing anything, naming the action.
    let (ok, _, err) = lab.try_run(&["-t", &t, "input", "move", "5", "5", "key", "ctrl+nope"]);
    assert!(!ok && err.contains("action 2 `key ctrl+nope`: unknown key 'nope'"), "{err}");
    let (ok, _, err) = lab.try_run(&["-t", &t, "input", "move", "10"]);
    assert!(!ok && err.contains("Y"), "{err}");
    assert_eq!(lab.json(&["-t", &t, "input", "move", "1", "1"])["pointer"], serde_json::json!([1.0, 1.0]));
    // Sleeping chains don't block other commands.
    let start = Instant::now();
    let mut slow = lab.cmd(&["-t", &t, "input", "sleep", "1500", "move", "2", "2"]).stdout(std::process::Stdio::null()).spawn().unwrap();
    wait_until("the chain to be pending", 10, || if lab.info().pending.inputs == 1 { Ok(()) } else { Err("not yet".into()) });
    lab.run(&["tree"]);
    assert_eq!(lab.info().pending.inputs, 1, "tree waited for the chain");
    assert!(slow.wait().unwrap().success());
    assert!(start.elapsed() >= Duration::from_millis(1500));
    // A chain whose client gave up (Ctrl-C) stops.
    let mut slow = lab.cmd(&["-t", &t, "input", "sleep", "3000", "move", "30", "30"]).stdout(std::process::Stdio::null()).spawn().unwrap();
    wait_until("the chain to be pending", 10, || if lab.info().pending.inputs == 1 { Ok(()) } else { Err("not yet".into()) });
    slow.kill().unwrap();
    let _ = slow.wait();
    wait_until("the server to drop the chain", 10, || if lab.info().pending.inputs == 0 { Ok(()) } else { Err("still pending".into()) });
    assert_eq!(lab.input(&t, &["sleep", "1"])["pointer"], serde_json::json!([2.0, 2.0]));

    // The server checks limits itself, whatever the client: a glide of 4e9 ms is refused.
    let mut c = guilab_proto::Client::connect(&lab.rt.join("guilab/control.sock")).unwrap();
    let target = guilab_proto::Target { token: Some(t.clone()), ..Default::default() };
    let actions = vec![guilab_proto::Action::Move { x: 1.0, y: 1.0, ms: 4_000_000_000 }];
    let req = guilab_proto::Request { proto: guilab_proto::PROTO, target, cmd: guilab_proto::SessionCmd::Input { actions }.into() };
    let err = c.call_as::<serde_json::Value>(&req).unwrap_err();
    assert!(err.contains("milliseconds must be 0 to 60000"), "{err}");

    // Taking over: the old token stops working.
    let t2 = lab.json(&["lock", "--force"])["token"].as_str().unwrap().to_string();
    assert_ne!(t, t2);
    let (ok, _, err) = lab.try_run(&["-t", &t, "input", "move", "1", "1"]);
    assert!(!ok && err.contains("someone else locked it"), "{err}");
    lab.run(&["-t", &t2, "unlock"]);
    let (ok, _, err) = lab.try_run(&["-t", &t2, "input", "move", "1", "1"]);
    assert!(!ok && err.contains("unlocked"), "{err}");
    lab.run(&["run", "--", "true"]);
    lab.run(&["end"]);
}

/// Whoever ends a lock (takeover, unlock) or gives up a chain, nothing stays held: keys
/// are released and an open touchpad gesture is ended.
#[test]
fn lock_end_releases_input() {
    if !gtk4() {
        return;
    }
    let lab = Lab::new();
    lab.run(&["new", "-s", "600x700"]);
    let log = lab.json(&["run", "--", &app("gtk_input.py")])["log"].as_str().unwrap().to_string();
    let l = wait_log(&log, "ready", 15);
    let t = lab.lock();
    let (ex, ey) = center(&l, "entry", 1.0);
    lab.input(&t, &["move", &ex, &ey, "click", "keydown", "shift"]);
    wait_log(&log, "key Shift_L", 5);
    // Taken over: the new holder doesn't inherit a held shift.
    let t2 = lab.json(&["lock", "--force"])["token"].as_str().unwrap().to_string();
    wait_log(&log, "keyup Shift_L", 5);
    // A chain given up mid-way (Ctrl-C) lets go too.
    let mut held = lab.cmd(&["-t", &t2, "input", "keydown", "ctrl", "sleep", "5000"]).stdout(std::process::Stdio::null()).spawn().unwrap();
    wait_log(&log, "key Control_L", 5);
    held.kill().unwrap();
    let _ = held.wait();
    wait_log(&log, "keyup Control_L", 5);

    // Unlocked mid-pinch: the app sees the gesture end.
    let (zx, zy) = center(&l, "zoom", 1.0);
    let mut pinch = lab.cmd(&["-t", &t2, "input", "move", &zx, &zy, "pinch", "3"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    wait_log(&log, "zoom 1.", 5);
    lab.run(&["-t", &t2, "unlock"]);
    if pinch.wait().unwrap().success() {
        eprintln!("lock_end_releases_input: the pinch finished before the unlock (slow machine?); not checking it");
        return;
    }
    wait_log(&log, "zoom end", 5);
}

#[test]
fn gtk_input_and_clipboard() {
    if !gtk4() {
        return;
    }
    let lab = Lab::new();
    // Fractional scale: coordinates are pixels, the app sees logical positions.
    let scale = 1.5;
    lab.run(&["new", "-s", "900x750", "--scale", "1.5"]);
    let log = lab.json(&["run", "--", &app("gtk_input.py")])["log"].as_str().unwrap().to_string();
    let l = wait_log(&log, "ready", 15);
    let c = |n: &str| center(&l, n, scale);
    let t = lab.lock();

    // Typing, including characters the keymap lacks.
    let (ex, ey) = c("entry");
    lab.input(&t, &["move", &ex, &ey, "click", "type", "Hé → ✓!"]);
    wait_log(&log, "entry 'Hé → ✓!'", 5);
    let many = "ĀāĂăĄąĆćĈĉĊċČčĎďĐđĒēĔĕĖėĘęĚěĜĝĞğĠġĢģĤĥĦħ";
    lab.input(&t, &["type", many]);
    wait_log(&log, &format!("entry 'Hé → ✓!{many}'"), 5);

    // Click, double-click and a right-click context menu, dismissed by clicking outside.
    let (bx, by) = c("button");
    lab.input(&t, &["move", &bx, &by, "click"]);
    wait_log(&log, "clicked 1", 5);
    let (ax, ay) = c("area");
    lab.input(&t, &["move", &ax, &ay, "click", "2", "click", "right"]);
    wait_log(&log, "pressed 1 2", 5);
    let tree = lab.wait_tree("s1", 5, |t| nodes(t).iter().any(|n| n["kind"] == "popup"));
    let popup = nodes(&tree).into_iter().find(|n| n["kind"] == "popup").unwrap().clone();
    // Pick the second item (surface-local coordinates), then reopen and click away. (`tree`
    // lists popups once they have a buffer: before, their height could still be 0.)
    let item_y = format!("{}", popup["height"].as_i64().unwrap() / 2);
    let info = lab.input(&t, &["on", &popup["id"].to_string(), "move", "30", &item_y, "click"]);
    // If the click closed the menu already, the reply still says where it landed.
    assert!(info["pressed_on"] == popup["id"] || info["under"] == popup["id"], "{info}");
    wait_log(&log, "menu 2", 5);
    lab.input(&t, &["move", &ax, &ay, "click", "right", "sleep", "300", "move", "20", "20", "click"]);
    lab.wait_tree("s1", 5, |t| !nodes(t).iter().any(|n| n["kind"] == "popup"));
    assert_eq!(wait_log(&log, "menu closed", 5).matches("menu closed").count(), 2);

    // Wheel and smooth scrolling.
    let (sx, sy) = c("scroll");
    lab.input(&t, &["move", &sx, &sy, "scroll", "0", "2"]);
    wait_log(&log, "vscroll", 5);
    lab.input(&t, &["scrollpx", "0", "-1000"]);
    wait_log(&log, "vscroll 0", 5);

    // Drag and drop (gliding, so the app sees motion after the press).
    let ((dx, dy), (tx, ty)) = (c("drag"), c("drop"));
    lab.input(&t, &["move", &dx, &dy, "down", "move", &tx, &ty, "300", "up"]);
    wait_log(&log, "dropped payload", 5);

    // Touch tap and a touchpad pinch.
    lab.input(&t, &["tap", &bx, &by]);
    wait_log(&log, "clicked 2", 5);
    let (zx, zy) = c("zoom");
    lab.input(&t, &["move", &zx, &zy, "pinch", "2"]);
    wait_log(&log, "zoom 2.0", 5);

    // Clipboard both ways: paste ours, copy the app's.
    lab.run(&["-t", &t, "clip", "set", "clip ü"]);
    assert_eq!(lab.json(&["clip", "get"]), "clip ü");
    lab.input(&t, &["move", &ex, &ey, "click", "key", "ctrl+a", "key", "ctrl+v"]);
    wait_log(&log, "entry 'clip ü'", 5);
    lab.input(&t, &["type", " copied", "key", "ctrl+a", "key", "ctrl+c"]);
    lab.wait_clip("clip ü copied", 5);
}

#[test]
fn gtk_focus_between_floating_windows() {
    if !gtk4() {
        return;
    }
    let lab = Lab::new();
    lab.run(&["new", "-s", "800x600"]);
    let log = lab.json(&["run", "--", &app("gtk_input.py"), "--floats"])["log"].as_str().unwrap().to_string();
    let l = wait_log(&log, "ready", 15);
    lab.wait_tree("s1", 10, |t| t.as_array().unwrap().len() == 3);
    let t = lab.lock();
    let (ex, ey) = center(&l, "entry", 1.0);
    let float = |title: &str| {
        let tree = lab.json(&["tree"]);
        rect(tree.as_array().unwrap().iter().find(|n| n["title"] == title).unwrap_or_else(|| panic!("no {title}: {tree}")))
    };
    let (a, b) = (float("float A"), float("float B"));
    // Cascaded: neither float can hide the other entirely.
    let inside = |i: (i64, i64, i64, i64), o: (i64, i64, i64, i64)| i.0 >= o.0 && i.1 >= o.1 && i.0 + i.2 <= o.0 + o.2 && i.1 + i.3 <= o.1 + o.3;
    assert!(!inside(a, b) && !inside(b, a), "A {a:?} B {b:?}");
    let s = |v: i64| v.to_string();
    let (acx, acy) = (a.0 + a.2 / 2, a.1 + a.3 / 2);
    lab.input(&t, &["move", &s(acx), &s(acy), "click", "type", "a", "move", &ex, &ey, "click", "type", "m"]);
    wait_log(&log, "A 'a'", 5);
    wait_log(&log, "entry 'm'", 5);
    // Focusing the tiled window keeps the floats above it.
    let tree = lab.json(&["tree"]);
    let titles: Vec<&str> = tree.as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap()).collect();
    assert_eq!(titles.last(), Some(&"guilab input"), "{titles:?}");
    assert_eq!(lab.input(&t, &["move", &s(acx), &s(acy)])["under"], tree[0]["id"]);
    // A title-bar drag moves a float (xdg_toplevel.move), and it stays put on resize.
    lab.input(&t, &["move", &s(acx), &s(a.1 + 12), "down", "move", &s(acx - 20), &s(a.1 + 2), "50", "move", &s(acx - 100), &s(a.1 - 68), "200", "up"]);
    let moved = float("float A");
    assert_eq!((moved.0, moved.1), (a.0 - 100, a.1 - 80), "A didn't follow the drag: {moved:?}");
    lab.run(&["-t", &t, "output", "-s", "900x700"]);
    let after = float("float A");
    assert_eq!((after.0, after.1), (moved.0 + 50, moved.1 + 50), "A lost its offset from the center: {after:?}");
}

#[test]
fn qt_menu_and_typing() {
    if !qt6() {
        return;
    }
    let lab = Lab::new();
    lab.run(&["new", "-s", "600x400"]);
    let log = lab.json(&["run", "--", &app("qt_app.py")])["log"].as_str().unwrap().to_string();
    let l = wait_log(&log, "ready", 15);
    let t = lab.lock();
    let (ex, ey) = center(&l, "edit", 1.0);
    let (lx, ly) = center(&l, "label", 1.0);
    lab.input(&t, &["move", &ex, &ey, "click", "type", "qt ☃ ü"]);
    wait_log(&log, "edit 'qt ☃ ü'", 5);
    // A QMenu only opens with an input serial.
    lab.input(&t, &["move", &lx, &ly, "click", "right"]);
    let tree = lab.wait_tree("s1", 5, |t| nodes(t).iter().any(|n| n["kind"] == "popup"));
    let popup = nodes(&tree).into_iter().find(|n| n["kind"] == "popup").unwrap().clone();
    let item3 = format!("{}", popup["height"].as_i64().unwrap() / 2);
    lab.input(&t, &["on", &popup["id"].to_string(), "move", "40", &item3, "click"]);
    wait_log(&log, "menu 3", 5);
    // Clicking the line edit closes a reopened menu; keys right after must reach the edit.
    lab.input(&t, &["on", "output", "move", &lx, &ly, "click", "right", "sleep", "300", "move", &ex, &ey, "click", "key", "End", "type", "!"]);
    wait_log(&log, "edit 'qt ☃ ü!'", 5);
    lab.wait_tree("s1", 5, |t| !nodes(t).iter().any(|n| n["kind"] == "popup"));
}

#[test]
fn kitty_types_unicode() {
    if !have("kitty") {
        return;
    }
    let lab = Lab::new();
    lab.run(&["new", "-s", "500x300"]);
    let out = lab.path("typed.txt");
    lab.run(&["run", "--", "kitty", "-o", "confirm_os_window_close=0", "sh", "-c", &format!("read l; printf %s \"$l\" > {out}")]);
    lab.wait_tree("s1", 15, |t| t.as_array().is_some_and(|a| !a.is_empty()));
    let t = lab.lock();
    // Keys before kitty started the shell are lost; the tty buffers them after that.
    wait_until("kitty's shell", 10, || if pgrep(&format!("^sh -c read l; printf %s .* > {out}$")).is_empty() { Err("not running".into()) } else { Ok(()) });
    lab.input(&t, &["type", "kitty é→✓ 日本 A!\\n"]);
    wait_until("the typed line in the file", 10, || {
        let got = std::fs::read_to_string(&out).unwrap_or_default();
        if got == "kitty é→✓ 日本 A!" { Ok(()) } else { Err(got) }
    });
}
