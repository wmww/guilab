//! Apps: run/ps/kill, and ending a session kills everything spawned in it.

use std::path::Path;

use crate::*;

#[test]
fn run_ps_kill() {
    let lab = Lab::new();
    lab.run(&["new"]);
    let p = lab.json(&["run", "--", "sh", "-c", "echo hello; exec sleep 100"]);
    assert_eq!(p["id"], 1);
    assert_eq!(p["state"], "running");
    lab.json(&["run", "--", "true"]);
    let ps = wait_until("app 2 exited", 10, || {
        let ps = lab.json(&["ps"]);
        if ps[1]["state"] == "exited" { Ok(ps) } else { Err(ps.to_string()) }
    });
    assert_eq!(ps[1]["code"], 0);
    assert_eq!(std::fs::read_to_string(ps[0]["log"].as_str().unwrap()).unwrap(), "hello\n");

    // `kill` replies once the app exited.
    lab.run(&["kill", "1"]);
    let ps = lab.json(&["ps"]);
    assert_eq!(ps[0]["state"], "signaled");
    assert_eq!(ps[0]["signal"], 15);
    let (ok, _, err) = lab.try_run(&["kill", "9"]);
    assert!(!ok && err.contains("no app 9"), "{err}");
}

#[test]
fn end_kills_descendants() {
    let lab = Lab::new();
    lab.run(&["new"]);
    // A double-forked daemon escapes the process group but keeps the session tag.
    let marker = format!("{}", 40000 + std::process::id() % 20000);
    lab.run(&["run", "--", "sh", "-c", &format!("setsid -f sleep {marker}1; exec sleep {marker}2")]);
    let pids = wait_until("both sleeps running", 10, || {
        let pids: Vec<u64> = [1, 2].iter().flat_map(|i| pgrep(&format!("sleep {marker}{i}"))).collect();
        if pids.len() == 2 { Ok(pids) } else { Err(format!("{pids:?}")) }
    });
    let dir = lab.json(&["ls"])[0]["dir"].as_str().unwrap().to_string();
    lab.run(&["end"]);
    wait_dead(&pids, "the session's processes to die");
    assert!(!Path::new(&dir).exists());
}

#[test]
fn server_stop_kills_apps() {
    let lab = Lab::new();
    lab.run(&["new"]);
    let pid = lab.json(&["run", "--", "sleep", "100"])["pid"].as_u64().unwrap();
    lab.run(&["server", "--stop"]);
    wait_dead(&[pid], "the app to die with the server");
}

/// Each session has a private D-Bus session bus; it ends with the session.
#[test]
fn private_session_bus() {
    if !have("dbus-daemon") || !have("dbus-send") {
        return;
    }
    let lab = Lab::new();
    let dir = lab.json(&["new"])["dir"].as_str().unwrap().to_string();
    let call = "echo $DBUS_SESSION_BUS_ADDRESS; dbus-send --session --print-reply --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.GetId";
    let log = lab.json(&["run", "--", "sh", "-c", call])["log"].as_str().unwrap().to_string();
    let out = wait_log(&log, "string \"", 5);
    assert!(out.starts_with(&format!("unix:path={dir}/bus")), "{out}");
    let conf = format!("{dir}/bus.conf");
    let daemon = pgrep(&conf);
    assert_eq!(daemon.len(), 1);
    lab.run(&["end"]);
    wait_dead(&daemon, "dbus-daemon to end with the session");
}

/// A session whose start fails after its D-Bus daemon started (here: `new -x` without
/// Xwayland) leaves no daemon behind.
#[test]
fn failed_new_leaves_no_bus() {
    let Ok(dbus) = which("dbus-daemon") else { return eprintln!("skipping: dbus-daemon not installed") };
    let lab = Lab::new();
    // The server autostarts with this PATH: dbus-daemon, but no Xwayland.
    let bin = lab.rt.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(dbus, bin.join("dbus-daemon")).unwrap();
    let (ok, _, err) = lab.try_run_env(&[("PATH", bin.to_str().unwrap())], &["new", "-x"]);
    assert!(!ok && err.contains("Xwayland"), "{err}");
    let conf = lab.rt.join("guilab/s");
    assert!(pgrep(&format!("dbus-daemon --config-file={}", conf.display())).is_empty(), "the session's dbus-daemon is still running");
}

fn which(prog: &str) -> Result<std::path::PathBuf, ()> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(prog)).find(|f| f.is_file())).ok_or(())
}
