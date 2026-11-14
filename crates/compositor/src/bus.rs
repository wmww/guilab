//! A private D-Bus session bus per session, so apps get uniqueness (GApplication), a bus
//! for their own services, and no "Unable to acquire session bus" noise, without reaching
//! the user's desktop bus.
//!
//! The bus has no service directories: nothing is activated on demand (no portals, settings
//! daemons or AT-SPI), so apps asking for those fail fast as before. The daemon carries the
//! session tag, so ending the session kills it like any app.

use std::{
    io::{BufRead, BufReader},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
};

use anyhow::{Context, bail};

/// The bus socket, in the session dir.
pub const BUS_SOCKET: &str = "bus";

fn config(socket: &Path) -> String {
    format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
        socket.display()
    )
}

/// Starts `dbus-daemon` for the session in `dir`; returns it and the bus address once the
/// bus is listening.
pub(crate) fn start(dir: &Path, tag_var: &str, tag: &str) -> anyhow::Result<(Child, String)> {
    let socket = dir.join(BUS_SOCKET);
    let conf = dir.join("bus.conf");
    std::fs::write(&conf, config(&socket)).with_context(|| format!("writing {}", conf.display()))?;
    let mut cmd = Command::new("dbus-daemon");
    cmd.arg(format!("--config-file={}", conf.display()))
        .args(["--nofork", "--nopidfile", "--print-address=1"])
        .env(tag_var, tag)
        .env("XDG_RUNTIME_DIR", dir)
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            // Like apps (see procs.rs): default signal mask, own group, dies with the server.
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::pthread_sigmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
            rustix::process::setsid()?;
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::TERM))?;
            Ok(())
        });
    }
    let mut child = cmd.spawn().context("running dbus-daemon")?;
    // It prints the address once it's listening.
    let mut line = String::new();
    let out = child.stdout.take().context("dbus-daemon stdout")?;
    if BufReader::new(out).read_line(&mut line).is_err() || line.trim().is_empty() {
        let _ = child.kill();
        let _ = child.wait();
        bail!("dbus-daemon didn't start");
    }
    Ok((child, line.trim().to_string()))
}
