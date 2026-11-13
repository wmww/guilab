//! Session lifecycle, resolution and TTL.

use crate::Lab;

#[test]
fn new_ls_end() {
    let lab = Lab::new();
    assert!(lab.run(&["ls"]).contains("no sessions"));
    let s = lab.json(&["new", "-s", "320x200"]);
    assert_eq!(s["name"], "s1");
    assert_eq!(s["output"]["width"], 320);
    assert!(std::path::Path::new(s["wayland_display"].as_str().unwrap()).exists());
    lab.run(&["new", "two", "--scale", "2"]);
    let ls = lab.json(&["ls"]);
    assert_eq!(ls.as_array().unwrap().len(), 2);

    // Two sessions and no --session: ambiguous.
    let (ok, _, err) = lab.try_run(&["ps"]);
    assert!(!ok && err.contains("--session"), "{err}");
    let (ok, _, err) = lab.try_run(&["new", "two"]);
    assert!(!ok && err.contains("exists"), "{err}");
    let (ok, _, err) = lab.try_run(&["-S", "nope", "ps"]);
    assert!(!ok && err.contains("no session 'nope'"), "{err}");

    assert_eq!(lab.json(&["-S", "two", "output"])["scale"], 2.0);
    lab.run(&["-S", "two", "end"]);
    // One left: it's the default now.
    assert!(lab.run(&["ps"]).contains("no apps"));
    lab.run(&["end"]);
    assert!(lab.run(&["ls"]).contains("no sessions"));
    assert!(!std::path::Path::new(s["dir"].as_str().unwrap()).exists());
}

#[test]
fn ttl_expires() {
    let lab = Lab::new();
    lab.run(&["new", "short", "--ttl", "1"]);
    lab.run(&["run", "--", "sleep", "100"]);
    let pid = lab.json(&["ps"])[0]["pid"].as_u64().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2500));
    assert!(lab.run(&["ls"]).contains("no sessions"));
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(!crate::cleanup::alive(pid), "app outlived its session");
}
