//! Short random ids. The first character tells the kind: rasters start with `r`, SVGs with
//! `s`, images with anything else.

use std::fmt;
use std::io::Read;

/// Id characters: no 0/1/i/l/o, so ids survive being read aloud or retyped.
pub const ALPHABET: &[u8; 31] = b"23456789abcdefghjkmnpqrstuvwxyz";
const MAX_LEN: usize = 7;

/// What an id names, from its first character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IdKind {
    Image,
    Raster,
    Svg,
}

impl IdKind {
    pub fn of(first: u8) -> IdKind {
        match first {
            b'r' => IdKind::Raster,
            b's' => IdKind::Svg,
            _ => IdKind::Image,
        }
    }

    /// "an image", "a raster", "an SVG".
    pub fn a(self) -> &'static str {
        match self {
            IdKind::Image => "an image",
            IdKind::Raster => "a raster",
            IdKind::Svg => "an SVG",
        }
    }
}

/// 4 characters normally; 5+ once a kind's 4-character space is half used (see `IdGen`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id {
    len: u8,
    b: [u8; MAX_LEN],
}

impl Id {
    /// Parses any well-formed id (kind not checked). Uppercase is accepted.
    pub fn parse(s: &str) -> Option<Id> {
        let s = s.as_bytes();
        if !(4..=MAX_LEN).contains(&s.len()) {
            return None;
        }
        let mut b = [0; MAX_LEN];
        for (i, c) in s.iter().enumerate() {
            let c = c.to_ascii_lowercase();
            if !ALPHABET.contains(&c) {
                return None;
            }
            b[i] = c;
        }
        Some(Id { len: s.len() as u8, b })
    }

    pub fn kind(&self) -> IdKind {
        IdKind::of(self.b[0])
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.b[..self.len as usize]).unwrap()
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a string isn't an id of the wanted kind. Display is a complete sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdError {
    Malformed(String),
    /// `want`: a noun phrase ("an image", "a raster").
    WrongKind { id: String, is: IdKind, want: &'static str },
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            IdError::Malformed(s) => write!(f, "\"{s}\" isn't an id (ids look like k3f9)"),
            IdError::WrongKind { id, is, want } => write!(f, "{id} is {}, not {want}", is.a()),
        }
    }
}

impl std::error::Error for IdError {}

fn parse_kind(s: &str, want: IdKind) -> Result<Id, IdError> {
    let id = Id::parse(s).ok_or_else(|| IdError::Malformed(s.to_string()))?;
    if id.kind() != want {
        return Err(IdError::WrongKind { id: id.to_string(), is: id.kind(), want: want.a() });
    }
    Ok(id)
}

/// An image id (never starts with `r` or `s`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageId(pub(crate) Id);

/// A raster (`r…`) or SVG (`s…`) id.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetId(pub(crate) Id);

impl ImageId {
    pub fn parse(s: &str) -> Result<ImageId, IdError> {
        parse_kind(s, IdKind::Image).map(ImageId)
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl AssetId {
    pub fn parse(s: &str) -> Result<AssetId, IdError> {
        let id = Id::parse(s).ok_or_else(|| IdError::Malformed(s.to_string()))?;
        match id.kind() {
            IdKind::Image => Err(IdError::WrongKind { id: id.to_string(), is: IdKind::Image, want: "a raster or SVG" }),
            _ => Ok(AssetId(id)),
        }
    }
    pub fn kind(&self) -> IdKind {
        self.0.kind()
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

macro_rules! show {
    ($t:ty) => {
        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(self.0.as_str())
            }
        }
        impl fmt::Debug for $t {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(self.0.as_str())
            }
        }
    };
}
show!(ImageId);
show!(AssetId);

/// Random id source (splitmix64 seeded from /dev/urandom). Uniqueness is the store's job.
pub struct IdGen {
    state: u64,
}

impl IdGen {
    pub fn new() -> IdGen {
        let mut seed = [0u8; 8];
        let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut seed)).is_ok();
        let mut s = u64::from_le_bytes(seed);
        if !ok {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            s = t.as_nanos() as u64 ^ (std::process::id() as u64) << 32;
        }
        IdGen { state: s }
    }

    /// Deterministic, for tests.
    pub fn seeded(seed: u64) -> IdGen {
        IdGen { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A random id of `kind` with `len` characters.
    pub fn gen_id(&mut self, kind: IdKind, len: usize) -> Id {
        assert!((4..=MAX_LEN).contains(&len));
        let mut b = [0; MAX_LEN];
        let mut r = self.next();
        b[0] = match kind {
            IdKind::Raster => b'r',
            IdKind::Svg => b's',
            IdKind::Image => {
                // Any first character except r and s.
                let firsts: Vec<u8> = ALPHABET.iter().copied().filter(|c| *c != b'r' && *c != b's').collect();
                let c = firsts[(r % firsts.len() as u64) as usize];
                r /= firsts.len() as u64;
                c
            }
        };
        for slot in b.iter_mut().take(len).skip(1) {
            if r < 31 {
                r = self.next();
            }
            *slot = ALPHABET[(r % 31) as usize];
            r /= 31;
        }
        Id { len: len as u8, b }
    }

    /// How many ids of `kind` exist with `len` characters.
    pub fn space(kind: IdKind, len: usize) -> u64 {
        let first = if kind == IdKind::Image { 29 } else { 1 };
        first * 31u64.pow(len as u32 - 1)
    }
}

impl Default for IdGen {
    fn default() -> Self {
        IdGen::new()
    }
}
