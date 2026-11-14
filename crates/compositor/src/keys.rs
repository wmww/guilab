//! Key names, keysym -> key lookup, and typing arbitrary Unicode.
//!
//! Characters the seat keymap has are typed with their real keys (plus Shift/AltGr as
//! needed), so raw-keycode clients see sensible keys. Characters it lacks are typed by
//! swapping in the seat keymap extended with those keysyms on spare keycodes (codes <= 255
//! with no symbols, so Xwayland can use them too), then swapping back. This replaces
//! smithay's `inject_text_keysyms`, whose throwaway keymap starts at code 9 = evdev
//! KEY_ESC, so raw-keycode clients (weston demos) see Escape.

use std::collections::HashMap;

use smithay::input::keyboard::{Keycode, Keysym, xkb};

/// Where a keysym lives: key + modifier keys to hold.
#[derive(Debug, Clone)]
pub struct KeyPos {
    pub code: Keycode,
    pub mods: Vec<Keycode>,
}

pub struct Keymap {
    /// Seat keymap as text (what clients get).
    pub text: String,
    by_sym: HashMap<u32, KeyPos>,
    /// Keycodes <= 255 without symbols, usable for extra keysyms.
    spare: Vec<u32>,
}

/// Friendly names -> keysym names.
fn alias(name: &str) -> &str {
    match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => "Control_L",
        "shift" => "Shift_L",
        "alt" => "Alt_L",
        "super" | "meta" | "win" | "logo" | "cmd" => "Super_L",
        "altgr" => "ISO_Level3_Shift",
        "enter" | "return" => "Return",
        "esc" | "escape" => "Escape",
        "backspace" => "BackSpace",
        "del" | "delete" => "Delete",
        "ins" | "insert" => "Insert",
        "pgup" | "pageup" => "Page_Up",
        "pgdn" | "pagedown" => "Page_Down",
        "space" => "space",
        "tab" => "Tab",
        "menu" => "Menu",
        "capslock" => "Caps_Lock",
        _ => name,
    }
}

/// Keysym for a key name: a single character, an alias, or an xkb keysym name.
pub fn keysym_for_name(name: &str) -> Option<Keysym> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return char_keysym(c);
    }
    let n = alias(name);
    let sym = xkb::keysym_from_name(n, xkb::KEYSYM_NO_FLAGS);
    if sym.raw() != 0 {
        return Some(sym);
    }
    let sym = xkb::keysym_from_name(n, xkb::KEYSYM_CASE_INSENSITIVE);
    (sym.raw() != 0).then_some(sym)
}

pub fn char_keysym(c: char) -> Option<Keysym> {
    let sym = match c {
        '\n' | '\r' => Keysym::Return,
        '\t' => Keysym::Tab,
        '\u{8}' => Keysym::BackSpace,
        '\u{1b}' => Keysym::Escape,
        c if c.is_control() => return None,
        c => xkb::utf32_to_keysym(c as u32),
    };
    (sym.raw() != 0).then_some(sym)
}

impl Keymap {
    pub fn new(text: String) -> anyhow::Result<Keymap> {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let km = xkb::Keymap::new_from_string(&ctx, text.clone(), xkb::KEYMAP_FORMAT_TEXT_V1, xkb::KEYMAP_COMPILE_NO_FLAGS)
            .ok_or_else(|| anyhow::anyhow!("seat keymap doesn't compile"))?;
        let (min, max) = (km.min_keycode().raw(), km.max_keycode().raw());

        // Modifier index -> a key that sets it (level 0 keysym).
        let mut level0: HashMap<u32, Keycode> = HashMap::new();
        for code in min..=max {
            let kc = Keycode::new(code);
            if let Some(s) = km.key_get_syms_by_level(kc, 0, 0).first() {
                level0.entry(s.raw()).or_insert(kc);
            }
        }
        let mut mod_keys: Vec<(u32, Keycode)> = Vec::new();
        for (name, sym) in [("Shift", Keysym::Shift_L), ("Mod5", Keysym::ISO_Level3_Shift), ("Control", Keysym::Control_L), ("Mod1", Keysym::Alt_L)] {
            let idx = km.mod_get_index(name);
            if idx != xkb::MOD_INVALID
                && let Some(kc) = level0.get(&sym.raw())
            {
                mod_keys.push((1 << idx, *kc));
            }
        }

        let mut by_sym: HashMap<u32, KeyPos> = HashMap::new();
        let mut spare = Vec::new();
        for code in min..=max {
            let kc = Keycode::new(code);
            let levels = if km.num_layouts_for_key(kc) == 0 { 0 } else { km.num_levels_for_key(kc, 0) };
            let mut any = false;
            for level in 0..levels {
                let syms = km.key_get_syms_by_level(kc, 0, level);
                any |= !syms.is_empty();
                let [sym] = syms else { continue };
                let mut masks = [0u32; 8];
                let n = km.key_get_mods_for_level(kc, 0, level, &mut masks);
                // The mask with the fewest modifiers that we can produce with modifier keys.
                let best = masks[..n.min(8)]
                    .iter()
                    .filter_map(|&m| {
                        let mut left = m;
                        let mut keys = Vec::new();
                        for (bit, k) in &mod_keys {
                            if left & bit != 0 {
                                keys.push(*k);
                                left &= !bit;
                            }
                        }
                        (left == 0).then_some(keys)
                    })
                    .min_by_key(Vec::len);
                if let Some(mods) = best {
                    // Prefer keys X can use (codes <= 255), then fewer modifiers.
                    let rank = |code: u32, mods: usize| (code > 255, mods);
                    let better = by_sym.get(&sym.raw()).is_none_or(|old| rank(old.code.raw(), old.mods.len()) > rank(code, mods.len()));
                    if better {
                        by_sym.insert(sym.raw(), KeyPos { code: kc, mods });
                    }
                }
            }
            if !any && (9..=255).contains(&code) {
                spare.push(code);
            }
        }
        spare.reverse();
        Ok(Keymap { text, by_sym, spare })
    }

    pub fn find(&self, sym: Keysym) -> Option<&KeyPos> {
        self.by_sym.get(&sym.raw())
    }

    /// Like `find`, but only keys X clients can receive (codes <= 255), for typing text.
    /// The `us` keymap has e.g. EuroSign only on code 443.
    pub fn find_typable(&self, sym: Keysym) -> Option<&KeyPos> {
        self.find(sym).filter(|k| k.code.raw() <= 255)
    }

    /// Resolves a key name to a key, or explains why not.
    pub fn key(&self, name: &str) -> Result<KeyPos, String> {
        let sym = keysym_for_name(name).ok_or_else(|| {
            format!("unknown key '{name}'; use names like a, A, 1, Return, Escape, Tab, BackSpace, Up, F5, ctrl, shift, alt, super, or xkb keysym names")
        })?;
        self.find(sym).cloned().ok_or_else(|| {
            format!("key '{name}' isn't on the keyboard (us layout); to enter that character use `type`")
        })
    }

    pub fn spare_count(&self) -> usize {
        self.spare.len()
    }

    /// The seat keymap text plus `extra` keysyms on spare keycodes, and where each landed.
    /// `extra.len()` must be <= `spare_count()`.
    pub fn extended(&self, extra: &[Keysym]) -> Option<(String, Vec<(Keysym, Keycode)>)> {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let km = xkb::Keymap::new_from_string(&ctx, self.text.clone(), xkb::KEYMAP_FORMAT_TEXT_V1, xkb::KEYMAP_COMPILE_NO_FLAGS)?;
        let mut names = String::new();
        let mut syms = String::new();
        let mut placed = Vec::new();
        for (sym, &code) in extra.iter().zip(&self.spare) {
            let kc = Keycode::new(code);
            let name = match km.key_get_name(kc) {
                Some(n) => n.to_string(),
                None => {
                    names.push_str(&format!("\t<GLX{code}> = {code};\n"));
                    format!("GLX{code}")
                }
            };
            let sname = xkb::keysym_get_name(*sym);
            syms.push_str(&format!("\tkey <{name}> {{ [ {sname} ] }};\n"));
            placed.push((*sym, kc));
        }
        // Splice into the serialized keymap (libxkbcommon's own format): names after the
        // `maximum` line of xkb_keycodes, symbols before the end of xkb_symbols.
        let t = &self.text;
        let kc_start = t.find("xkb_keycodes")?;
        let max_line = kc_start + t[kc_start..].find("maximum")?;
        let names_at = max_line + t[max_line..].find('\n')? + 1;
        let sym_start = t.find("xkb_symbols")?;
        let syms_at = sym_start + t[sym_start..].find("\n};")? + 1;
        let mut out = String::with_capacity(t.len() + names.len() + syms.len());
        out.push_str(&t[..names_at]);
        out.push_str(&names);
        out.push_str(&t[names_at..syms_at]);
        out.push_str(&syms);
        out.push_str(&t[syms_at..]);
        Some((out, placed))
    }
}
