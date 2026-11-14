//! The desktop's light/dark preference: `org.freedesktop.appearance color-scheme` from the
//! XDG settings portal (what GTK/libadwaita follow), on this process's session bus.
//!
//! A tiny blocking D-Bus client on a thread (one per process): connect, `Hello`, subscribe
//! to `SettingChanged`, read the value, then wait for changes. [`color_scheme`] returns the
//! latest. No D-Bus library: the few messages needed are marshalled here.

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    sync::{
        OnceLock,
        atomic::{AtomicU8, Ordering},
    },
};

/// 0 = unknown / no preference, 1 = dark, 2 = light (the portal's values).
static SCHEME: AtomicU8 = AtomicU8::new(0);
static STARTED: OnceLock<()> = OnceLock::new();

/// The desktop's preference, if it states one. Starts the watcher on first call.
pub fn color_scheme() -> Option<egui::Theme> {
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new().name("guilab-portal".into()).spawn(|| {
            if let Err(e) = watch() {
                tracing::debug!("no color scheme from the portal: {e}");
            }
        });
    });
    match SCHEME.load(Ordering::Relaxed) {
        1 => Some(egui::Theme::Dark),
        2 => Some(egui::Theme::Light),
        _ => None,
    }
}

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
const NS: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";

fn watch() -> std::io::Result<()> {
    let mut bus = Bus::connect()?;
    bus.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "Hello", "", &[])?;
    let rule = format!("type='signal',interface='{SETTINGS}',member='SettingChanged',path='{PATH}',arg0='{NS}',arg1='{KEY}'");
    bus.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "AddMatch", "s", &[Arg::Str(&rule)])?;
    let read = bus.call(PORTAL, PATH, SETTINGS, "ReadOne", "ss", &[Arg::Str(NS), Arg::Str(KEY)])?;
    // ReadOne is newer; old portals only have Read (a variant inside a variant).
    let read = if read.is_error() { bus.call(PORTAL, PATH, SETTINGS, "Read", "ss", &[Arg::Str(NS), Arg::Str(KEY)])? } else { read };
    let v = read.body_u32(0);
    tracing::debug!("portal color-scheme: {v:?}");
    if let Some(v) = v {
        SCHEME.store(v as u8, Ordering::Relaxed);
    }
    loop {
        let m = bus.read()?;
        if m.kind == SIGNAL && m.member.as_deref() == Some("SettingChanged") {
            // (s namespace, s key, v value)
            let mut r = Reader { b: &m.body, pos: 0 };
            let (ns, key) = (r.string(), r.string());
            if ns.as_deref() == Some(NS) && key.as_deref() == Some(KEY)
                && let Some(v) = r.variant_u32()
            {
                SCHEME.store(v as u8, Ordering::Relaxed);
            }
        }
    }
}

const CALL: u8 = 1;
const ERROR: u8 = 3;
const SIGNAL: u8 = 4;

enum Arg<'a> {
    Str(&'a str),
}

struct Msg {
    kind: u8,
    reply_to: Option<u32>,
    member: Option<String>,
    body: Vec<u8>,
}

impl Msg {
    fn is_error(&self) -> bool {
        self.kind == ERROR
    }

    /// A body that starts with a variant holding a u32 (variants may nest).
    fn body_u32(&self, at: usize) -> Option<u32> {
        Reader { b: &self.body, pos: at }.variant_u32()
    }
}

struct Bus {
    r: BufReader<UnixStream>,
    w: UnixStream,
    serial: u32,
}

fn pad(b: &mut Vec<u8>, align: usize) {
    while !b.len().is_multiple_of(align) {
        b.push(0);
    }
}

fn put_str(b: &mut Vec<u8>, s: &str) {
    pad(b, 4);
    b.extend((s.len() as u32).to_le_bytes());
    b.extend(s.as_bytes());
    b.push(0);
}

fn put_sig(b: &mut Vec<u8>, s: &str) {
    b.push(s.len() as u8);
    b.extend(s.as_bytes());
    b.push(0);
}

impl Bus {
    fn connect() -> std::io::Result<Bus> {
        let addr = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().or_else(|| std::env::var("XDG_RUNTIME_DIR").ok().map(|d| format!("unix:path={d}/bus")));
        let addr = addr.ok_or_else(|| std::io::Error::other("no session bus address"))?;
        let mut last = std::io::Error::other("no usable unix address");
        for a in addr.split(';') {
            let Some(rest) = a.strip_prefix("unix:") else { continue };
            let kv = |k: &str| rest.split(',').find_map(|p| p.strip_prefix(k).and_then(|v| v.strip_prefix('=')));
            let stream = if let Some(path) = kv("path") {
                UnixStream::connect(path)
            } else if let Some(name) = kv("abstract") {
                use std::os::linux::net::SocketAddrExt;
                std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).and_then(|sa| UnixStream::connect_addr(&sa))
            } else {
                continue;
            };
            match stream {
                Ok(s) => return Bus::auth(s),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn auth(mut s: UnixStream) -> std::io::Result<Bus> {
        let uid: String = rustix::process::getuid().as_raw().to_string().bytes().map(|b| format!("{b:02x}")).collect();
        s.write_all(format!("\0AUTH EXTERNAL {uid}\r\n").as_bytes())?;
        let mut r = BufReader::new(s.try_clone()?);
        let mut line = String::new();
        r.read_line(&mut line)?;
        if !line.starts_with("OK ") {
            return Err(std::io::Error::other(format!("bus auth: {}", line.trim())));
        }
        s.write_all(b"BEGIN\r\n")?;
        Ok(Bus { r, w: s, serial: 0 })
    }

    /// Sends a method call and returns its reply or error (signals before it are dropped).
    fn call(&mut self, dest: &str, path: &str, iface: &str, member: &str, sig: &str, args: &[Arg]) -> std::io::Result<Msg> {
        self.serial += 1;
        let mut body = Vec::new();
        for a in args {
            match a {
                Arg::Str(s) => put_str(&mut body, s),
            }
        }
        let mut m = vec![b'l', CALL, 0, 1];
        m.extend((body.len() as u32).to_le_bytes());
        m.extend(self.serial.to_le_bytes());
        let mut fields = Vec::new();
        let mut field = |code: u8, ty: &str, write: &dyn Fn(&mut Vec<u8>)| {
            pad(&mut fields, 8);
            fields.push(code);
            put_sig(&mut fields, ty);
            write(&mut fields);
        };
        field(1, "o", &|b| put_str(b, path));
        field(2, "s", &|b| put_str(b, iface));
        field(3, "s", &|b| put_str(b, member));
        field(6, "s", &|b| put_str(b, dest));
        if !sig.is_empty() {
            field(8, "g", &|b| put_sig(b, sig));
        }
        // Field offsets were computed from the array's start, which sits at offset 16: also
        // 8-aligned, so the padding is right.
        m.extend((fields.len() as u32).to_le_bytes());
        m.extend(fields);
        pad(&mut m, 8);
        m.extend(body);
        self.w.write_all(&m)?;
        loop {
            let reply = self.read()?;
            if reply.reply_to == Some(self.serial) {
                return Ok(reply);
            }
        }
    }

    fn read(&mut self) -> std::io::Result<Msg> {
        let mut head = [0u8; 16];
        self.r.read_exact(&mut head)?;
        if head[0] != b'l' {
            return Err(std::io::Error::other("big-endian bus messages aren't supported"));
        }
        let u32_at = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        let body_len = u32_at(&head, 4) as usize;
        let fields_len = u32_at(&head, 12) as usize;
        let padded = fields_len.div_ceil(8) * 8;
        let mut fields = vec![0u8; padded];
        self.r.read_exact(&mut fields)?;
        let mut body = vec![0u8; body_len];
        self.r.read_exact(&mut body)?;
        let mut msg = Msg { kind: head[1], reply_to: None, member: None, body };
        // Fields: array of (byte code, variant); offsets relative to the array (8-aligned).
        let mut r = Reader { b: &fields[..fields_len], pos: 0 };
        while r.pos < fields_len {
            r.align(8);
            let Some(code) = r.byte() else { break };
            let Some(sig) = r.sig() else { break };
            match (code, sig.as_str()) {
                (5, "u") => msg.reply_to = r.u32(),
                (3, "s") => msg.member = r.string(),
                (_, "s" | "o") => drop(r.string()),
                (_, "g") => drop(r.sig()),
                (_, "u") => drop(r.u32()),
                _ => break,
            }
        }
        Ok(msg)
    }
}

/// Reads marshalled values; `pos` is relative to an 8-aligned start.
struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn align(&mut self, a: usize) {
        self.pos = self.pos.div_ceil(a) * a;
    }

    fn byte(&mut self) -> Option<u8> {
        let v = *self.b.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }

    fn u32(&mut self) -> Option<u32> {
        self.align(4);
        let v = u32::from_le_bytes(self.b.get(self.pos..self.pos + 4)?.try_into().ok()?);
        self.pos += 4;
        Some(v)
    }

    fn string(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        let s = String::from_utf8_lossy(self.b.get(self.pos..self.pos + n)?).into_owned();
        self.pos += n + 1;
        Some(s)
    }

    fn sig(&mut self) -> Option<String> {
        let n = self.byte()? as usize;
        let s = String::from_utf8_lossy(self.b.get(self.pos..self.pos + n)?).into_owned();
        self.pos += n + 1;
        Some(s)
    }

    /// A variant holding a u32, possibly inside more variants.
    fn variant_u32(&mut self) -> Option<u32> {
        match self.sig()?.as_str() {
            "u" => self.u32(),
            "v" => self.variant_u32(),
            _ => None,
        }
    }
}
