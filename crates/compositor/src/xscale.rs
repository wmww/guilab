//! Xwayland HiDPI and X client info over our own X connection.
//!
//! X clients render at the output scale: Xwayland's client scale maps X pixels to logical
//! ones, so an X window at scale 2 has twice the pixels. Toolkits learn the scale from
//! XSETTINGS (GTK3: `Gdk/WindowScalingFactor`, `Gdk/UnscaledDPI`; fonts: `Xft/DPI`) and
//! `Xft.dpi` in RESOURCE_MANAGER (Qt, Xft apps, read at their startup). Apps that ignore
//! both (core X fonts, xeyes) draw at their pixel sizes, i.e. small on HiDPI outputs.
//!
//! Pids: `_NET_WM_PID` if the client set it, else the X-Resource extension's client pid.

use smithay::xwayland::xwm::settings::Value;
use x11rb::{
    connection::{Connection, RequestConnection},
    wrapper::ConnectionExt as _,
    protocol::{
        res::{ClientIdMask, ClientIdSpec, ConnectionExt as _},
        xproto::{AtomEnum, ConnectionExt as _, PropMode},
    },
    rust_connection::RustConnection,
};

use crate::session::Session;

/// Our own client connection to the session's X server.
pub struct XConn {
    conn: RustConnection,
    root: u32,
    /// X-Resource is available.
    res: bool,
}

impl XConn {
    pub fn connect(display: u32) -> anyhow::Result<XConn> {
        let (conn, screen) = RustConnection::connect(Some(&format!(":{display}")))?;
        let root = conn.setup().roots[screen].root;
        let res = conn.extension_information(x11rb::protocol::res::X11_EXTENSION_NAME)?.is_some();
        Ok(XConn { conn, root, res })
    }

    /// Sets `Xft.dpi` in the root window's RESOURCE_MANAGER, keeping other resources.
    fn set_xft_dpi(&self, dpi: u32) -> anyhow::Result<()> {
        let cur = self.conn.get_property(false, self.root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 20)?.reply()?;
        let text = String::from_utf8_lossy(&cur.value);
        let mut lines: Vec<String> = text.lines().filter(|l| !l.starts_with("Xft.dpi:")).map(str::to_string).collect();
        lines.push(format!("Xft.dpi:\t{dpi}"));
        let value = lines.join("\n") + "\n";
        self.conn.change_property8(PropMode::REPLACE, self.root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, value.as_bytes())?;
        self.conn.flush()?;
        Ok(())
    }

    /// The pid of the client that owns X window `xid` (X-Resource).
    pub fn pid(&self, xid: u32) -> Option<u32> {
        if !self.res {
            return None;
        }
        let spec = ClientIdSpec { client: xid, mask: ClientIdMask::LOCAL_CLIENT_PID };
        let reply = self.conn.res_query_client_ids(&[spec]).ok()?.reply().ok()?;
        reply.ids.iter().find(|id| id.spec.mask.contains(ClientIdMask::LOCAL_CLIENT_PID)).and_then(|id| id.value.first().copied())
    }
}

impl Session {
    /// Tells X clients the output scale (client scale, XSETTINGS, Xft.dpi). Call when
    /// Xwayland is ready and whenever the scale changes (before relayout).
    pub(crate) fn x_apply_scale(&mut self) {
        let s = self.output.current_scale().fractional_scale();
        if let Some(client) = &self.xw.client {
            use smithay::wayland::compositor::CompositorHandler;
            self.client_compositor_state(client).set_client_scale(s);
        }
        // GTK3 scales by integers; the rest goes through the font DPI.
        let int = (s.floor() as i32).max(1);
        let dpi = 96.0 * s;
        if let Some(wm) = &mut self.xw.wm {
            let settings = [
                ("Xft/DPI".to_string(), Value::Integer((dpi * 1024.0).round() as i32)),
                ("Gdk/WindowScalingFactor".to_string(), Value::Integer(int)),
                ("Gdk/UnscaledDPI".to_string(), Value::Integer((dpi * 1024.0 / int as f64).round() as i32)),
            ];
            if let Err(e) = wm.set_xsettings(settings.into_iter()) {
                tracing::warn!("{}: XSETTINGS: {e}", self.name);
            }
            // The root cursor: what X windows that set none show (else Xwayland's X).
            if let Some(c) = crate::cursor::load_named(smithay::input::pointer::CursorIcon::Default, s) {
                let size = (c.width.min(u16::MAX as u32) as u16, c.height.min(u16::MAX as u32) as u16);
                if let Err(e) = wm.set_cursor(&c.rgba, size.into(), (c.hotspot.0.max(0) as u16, c.hotspot.1.max(0) as u16).into()) {
                    tracing::warn!("{}: X root cursor: {e}", self.name);
                }
            }
        }
        if let Some(c) = &self.xw.conn
            && let Err(e) = c.set_xft_dpi(dpi.round() as u32)
        {
            tracing::warn!("{}: setting Xft.dpi: {e:#}", self.name);
        }
    }
}
