//! A complete host for the chat without guilab: owns the images (synthetic screenshots),
//! the pending list, which image is "open", and undo of removals. The shots example and the
//! tests use it; real hosts do the same.

use std::collections::HashMap;

use egui::{Color32, ColorImage, Context, Key, Modifiers, TextureHandle, TextureId, TextureOptions, Ui};
use guilab_kit::History;

use crate::{Chat, ChatEvent, ChatOut, Item, SlotId, SlotState, Status, Thumbs};

pub struct DemoImages {
    pub images: HashMap<u32, ColorImage>,
    textures: HashMap<u32, TextureHandle>,
    /// The open image and the slot it was opened from.
    pub open: Option<(SlotId, u32)>,
    /// Images with no texture (placeholders), e.g. expired.
    pub missing: Vec<u32>,
}

impl Thumbs<u32> for DemoImages {
    fn aspect(&mut self, key: &u32) -> f32 {
        self.images.get(key).map_or(1.0, |i| i.size[0] as f32 / i.size[1] as f32)
    }

    fn texture(&mut self, ctx: &Context, key: &u32, _height_px: f32) -> Option<TextureId> {
        if self.missing.contains(key) {
            return None;
        }
        let img = self.images.get(key)?;
        Some(self.textures.entry(*key).or_insert_with(|| ctx.load_texture(format!("demo-{key}"), img.clone(), TextureOptions::LINEAR)).id())
    }

    fn state(&self, slot: SlotId, key: &u32) -> SlotState {
        match self.open {
            Some((s, k)) if k == *key && s == slot => SlotState::Open,
            _ => SlotState::Shown,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Op {
    Remove { at: usize, key: u32 },
}

pub struct DemoHost {
    pub chat: Chat<u32>,
    pub thumbs: DemoImages,
    pub pending: Vec<u32>,
    pub history: History<Op>,
    pub status: Option<Status>,
    pub last: ChatOut<u32>,
    /// Every event, in order (tests read it).
    pub log: Vec<ChatEvent<u32>>,
    next: u32,
}

impl Default for DemoHost {
    fn default() -> Self {
        DemoHost::new()
    }
}

impl DemoHost {
    pub fn new() -> DemoHost {
        let mut chat = Chat::new("demo-chat");
        chat.hint = "What's next?".into();
        chat.empty_hint = "Messages, and what the agent does, show up here.".into();
        DemoHost {
            chat,
            thumbs: DemoImages { images: HashMap::new(), textures: HashMap::new(), open: None, missing: Vec::new() },
            pending: Vec::new(),
            history: History::new(),
            status: None,
            last: ChatOut::default(),
            log: Vec::new(),
            next: 1,
        }
    }

    /// Adds a synthetic screenshot `w`x`h` (hue from `seed`) and returns its key.
    pub fn image(&mut self, w: usize, h: usize, seed: u32) -> u32 {
        let k = self.next;
        self.next += 1;
        self.thumbs.images.insert(k, fake_screenshot(w, h, seed));
        k
    }

    /// A conversation with every kind of item.
    pub fn sample(&mut self) {
        let a = self.image(800, 500, 0);
        let b = self.image(640, 400, 1);
        let s1 = self.image(800, 500, 2);
        let s2 = self.image(800, 500, 3);
        let tall = self.image(300, 900, 4);
        let c = &mut self.chat;
        c.push(Item::me("14:02", "The sidebar spacing looks off, and the save button is too close to the edge.").with([a]));
        c.push(Item::activity("14:02", "guilab poll"));
        c.push(Item::activity("14:02", "guilab work \"fixing sidebar spacing\""));
        c.push(Item::activity("14:03", "cargo build --release 2>&1 | tail -n 20"));
        c.push(Item::activity("14:03", "guilab screenshot").with([s1]).faint());
        c.push(Item::activity("14:04", "guilab image make --from k3f9 'yellow arrow 900,200 to 260,180' 'red box 600,380 120x60'").with([s2]).faint());
        c.push(Item::them("Agent", "14:04", "Fixed the gap: it was 30px, now 16px like the rest. Here's before and after."));
        c.push(Item::them("Agent", "14:04", "").with([b, tall]));
        c.push(Item::me("14:05", "Better. Can you also check the dialog?"));
        c.push(Item::activity("14:05", "guilab poll"));
        self.status = Some(Status { text: "checking the dialog".into(), elapsed: 75, note: None });
    }

    /// Adds a pending attachment (as if drawn on).
    pub fn attach(&mut self, w: usize, h: usize, seed: u32) -> u32 {
        let k = self.image(w, h, seed);
        self.pending.push(k);
        k
    }

    pub fn undo(&mut self) {
        if let Some(ops) = self.history.undo() {
            for op in ops.iter().rev() {
                let Op::Remove { at, key } = op;
                self.pending.insert((*at).min(self.pending.len()), *key);
            }
        }
    }

    pub fn redo(&mut self) {
        if let Some(ops) = self.history.redo() {
            for op in ops {
                let Op::Remove { key, .. } = op;
                self.remove(key);
            }
        }
    }

    fn remove(&mut self, key: u32) {
        self.pending.retain(|k| *k != key);
        if matches!(self.thumbs.open, Some((SlotId::Composer, k)) if k == key) {
            self.thumbs.open = None;
        }
    }

    /// "Edits" the open past image: a pending copy appears (open, washed), and the feed
    /// slot quietly shows its image again.
    pub fn edit_open(&mut self) {
        let Some((slot @ SlotId::Feed { .. }, key)) = self.thumbs.open else { return };
        let img = self.thumbs.images[&key].clone();
        let k = self.next;
        self.next += 1;
        self.thumbs.images.insert(k, img);
        self.pending.push(k);
        self.chat.cross_fade(slot, key);
        self.thumbs.open = Some((SlotId::Composer, k));
    }

    /// The chat filling `ui`, with the open image (if any) noted at the top.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.chat.set_status(self.status.clone());
        let out = self.chat.show(ui, &self.pending, &mut self.thumbs);
        let mut undo = false;
        let mut redo = false;
        for e in &out.events {
            match e {
                ChatEvent::Open { slot, key, .. } => {
                    // Clicking the open image's own slot closes it.
                    self.thumbs.open = if self.thumbs.open == Some((*slot, *key)) { None } else { Some((*slot, *key)) };
                }
                ChatEvent::Started => {}
                ChatEvent::Remove { key } => {
                    if let Some(at) = self.pending.iter().position(|k| k == key) {
                        self.history.push(Op::Remove { at, key: *key });
                        self.remove(*key);
                    }
                }
                ChatEvent::Send { text } => {
                    let att = std::mem::take(&mut self.pending);
                    self.history.clear();
                    if matches!(self.thumbs.open, Some((SlotId::Composer, _))) {
                        self.thumbs.open = None;
                    }
                    self.chat.push(Item::me("14:06", text.clone()).with(att));
                }
                ChatEvent::Undo => undo = true,
                ChatEvent::Redo => redo = true,
            }
        }
        // Host undo when no text box has the keys.
        if !ui.ctx().egui_wants_keyboard_input() {
            ui.input_mut(|i| {
                redo |= i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y);
                undo |= i.consume_key(Modifiers::COMMAND, Key::Z);
            });
        }
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
        self.log.extend(out.events.iter().cloned());
        self.last = out;
    }
}

/// A synthetic app screenshot: title bar, sidebar, lines of "text", a button, a red box mark.
pub fn fake_screenshot(w: usize, h: usize, seed: u32) -> ColorImage {
    let hue = [[53, 132, 228], [46, 160, 90], [200, 90, 60], [140, 90, 200], [210, 160, 40]][seed as usize % 5];
    let mut img = ColorImage::new([w, h], vec![Color32::from_rgb(250, 250, 250); w * h]);
    let mut fill = |x0: f32, y0: f32, x1: f32, y1: f32, c: [u8; 3]| {
        let (x0, y0, x1, y1) = ((x0 * w as f32) as usize, (y0 * h as f32) as usize, ((x1 * w as f32) as usize).min(w), ((y1 * h as f32) as usize).min(h));
        for y in y0..y1 {
            for x in x0..x1 {
                img.pixels[y * w + x] = Color32::from_rgb(c[0], c[1], c[2]);
            }
        }
    };
    fill(0.0, 0.0, 1.0, 0.08, [232, 232, 236]);
    fill(0.0, 0.08, 0.25, 1.0, [242, 242, 245]);
    for i in 0..6 {
        let y = 0.14 + i as f32 * 0.07;
        fill(0.03, y, 0.12 + ((i * 7 + seed as usize) % 9) as f32 * 0.012, y + 0.025, [150, 150, 158]);
    }
    fill(0.02, 0.26, 0.23, 0.31, hue);
    for i in 0..10 {
        let y = 0.14 + i as f32 * 0.06;
        let len = 0.3 + ((i * 13 + seed as usize * 5) % 11) as f32 * 0.035;
        fill(0.3, y, 0.3 + len, y + 0.022, [120, 120, 128]);
    }
    fill(0.78, 0.86, 0.96, 0.94, hue);
    // A red box mark, as drawn by a human.
    let (bx0, by0, bx1, by1) = (0.28, 0.1 + seed as f32 % 3.0 * 0.15, 0.7, 0.28 + seed as f32 % 3.0 * 0.15);
    let t = 0.008;
    for (a, b, c, d) in [(bx0, by0, bx1, by0 + t * 1.6), (bx0, by1 - t * 1.6, bx1, by1), (bx0, by0, bx0 + t, by1), (bx1 - t, by0, bx1, by1)] {
        fill(a, b, c, d, [230, 40, 40]);
    }
    img
}
