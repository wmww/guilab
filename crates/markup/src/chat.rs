//! The conversation panel: a feed (human messages, agent messages, activity lines) above
//! a message box with the queued drawings. The host pushes [`FeedItem`]s in and gets
//! [`Outgoing`] messages back; it decides what happens to them.

use std::{collections::VecDeque, sync::Arc};

use egui::{Color32, ColorImage, Id, Key, Modifiers, RichText, Sense, TextureHandle, TextureOptions, Ui, vec2};

use crate::{
    raster::downscale,
    sketch::{Annotated, Sketch, THUMB},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    /// The person using this UI.
    Human,
    Agent,
    /// Activity (e.g. a command the agent ran): one compact line.
    Log,
}

#[derive(Clone)]
pub struct FeedItem {
    pub who: Who,
    /// Shown as is (e.g. "14:05:31").
    pub time: String,
    pub text: String,
    pub images: Vec<Arc<ColorImage>>,
}

/// A message the human sent: text plus the queued drawings (and the open one).
pub struct Outgoing {
    pub text: String,
    pub images: Vec<Annotated>,
}

struct Item {
    feed: FeedItem,
    thumbs: Vec<Option<(TextureHandle, [usize; 2])>>,
    full: Vec<Option<TextureHandle>>,
}

pub struct Chat {
    id: Id,
    items: VecDeque<Item>,
    draft: String,
    /// Oldest items are dropped past this.
    pub max_items: usize,
    scroll_to_end: bool,
    /// The feed was scrolled to the end last frame (new items then keep it there).
    at_end: bool,
}

impl Chat {
    pub fn new(id: impl Into<Id>) -> Chat {
        Chat { id: id.into(), items: VecDeque::new(), draft: String::new(), max_items: 2000, scroll_to_end: true, at_end: true }
    }

    pub fn push(&mut self, feed: FeedItem) {
        let n = feed.images.len();
        self.items.push_back(Item { feed, thumbs: vec![None; n], full: vec![None; n] });
        self.scroll_to_end |= self.at_end;
        while self.items.len() > self.max_items {
            self.items.pop_front();
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The draft in the message box (e.g. for a host to prefill or inspect).
    pub fn draft_mut(&mut self) -> &mut String {
        &mut self.draft
    }

    /// Shows the feed and the message box in `ui`'s remaining space. Returns a message when
    /// the human sends one (Enter or Send); it takes `sketch`'s queue and open drawing.
    pub fn show(&mut self, ui: &mut Ui, sketch: &mut Sketch) -> Option<Outgoing> {
        let mut sent = None;
        egui::Panel::bottom(self.id.with("input")).show_separator_line(false).frame(egui::Frame::NONE).show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                sketch.queue_strip(ui);
                if sketch.drawing_open() {
                    ui.weak("+ open drawing").on_hover_text("Sent with the next message");
                }
                let send_w = 56.0;
                let edit = egui::TextEdit::multiline(&mut self.draft)
                    .id(self.id.with("draft"))
                    .hint_text("Message the agent (Enter: send, Shift+Enter: new line)")
                    .desired_rows(1)
                    .desired_width(ui.available_width() - send_w - 8.0);
                // Plain Enter sends; take it before the text box turns it into a newline.
                let has_focus = ui.memory(|m| m.has_focus(self.id.with("draft")));
                let enter = has_focus && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
                ui.add(edit);
                let click = ui.add_sized(vec2(send_w, 24.0), egui::Button::new("Send")).clicked();
                let has_images = sketch.queued() > 0 || sketch.drawing_open();
                if (enter || click) && (!self.draft.trim().is_empty() || has_images) {
                    sent = Some(Outgoing { text: self.draft.trim().to_string(), images: sketch.take_all() });
                    self.draft.clear();
                    // Follow the conversation again (the sent message arrives as a feed item).
                    self.at_end = true;
                    self.scroll_to_end = true;
                }
            });
            ui.add_space(4.0);
        });

        let id = self.id;
        let out = egui::ScrollArea::vertical().id_salt(id.with("feed")).auto_shrink(false).show(ui, |ui| {
            if self.items.is_empty() {
                ui.weak("Messages, and what the agent does, show up here.");
            }
            for item in self.items.iter_mut() {
                show_item(ui, item);
            }
            if std::mem::take(&mut self.scroll_to_end) {
                ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
            }
        });
        self.at_end = out.state.offset.y + out.inner_rect.height() >= out.content_size.y - 8.0;
        sent
    }
}

fn show_item(ui: &mut Ui, item: &mut Item) {
    let f = &item.feed;
    match f.who {
        Who::Log => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(&f.time).small().weak());
                ui.label(RichText::new(&f.text).small().monospace().weak());
            });
        }
        Who::Human | Who::Agent => {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                let (name, color) = match f.who {
                    Who::Human => ("You", Color32::from_rgb(110, 170, 255)),
                    _ => ("Agent", Color32::from_rgb(120, 210, 140)),
                };
                ui.label(RichText::new(name).strong().color(color));
                ui.label(RichText::new(&f.time).small().weak());
            });
            if !f.text.is_empty() {
                ui.add(egui::Label::new(&f.text).selectable(true));
            }
            if !f.images.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for (i, img) in f.images.iter().enumerate() {
                        let (tex, size) = item.thumbs[i].get_or_insert_with(|| {
                            let t = downscale(img, THUMB);
                            let size = t.size;
                            (ui.ctx().load_texture("feed-thumb", t, TextureOptions::LINEAR), size)
                        });
                        let s = vec2(size[0] as f32, size[1] as f32) * (80.0 / size[1] as f32);
                        let resp = ui.add(egui::Image::new((tex.id(), s)).sense(Sense::hover()));
                        let full = &mut item.full[i];
                        resp.on_hover_ui(|ui| {
                            let tex = full.get_or_insert_with(|| ui.ctx().load_texture("feed-image", (**img).clone(), TextureOptions::LINEAR));
                            let size = vec2(img.size[0] as f32, img.size[1] as f32);
                            let k = (720.0 / size.x).min(480.0 / size.y).min(1.0);
                            ui.image((tex.id(), size * k));
                        });
                    }
                });
            }
            ui.add_space(2.0);
        }
    }
}
