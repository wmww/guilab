//! The conversation panel: a feed (human messages, agent messages, activity lines) above
//! a message box with the queued drawings. The host pushes [`FeedItem`]s in and gets
//! [`Outgoing`] messages back; it decides what happens to them.

use std::{collections::VecDeque, sync::Arc};

use egui::{Color32, ColorImage, Id, RichText, Sense, TextureHandle, TextureOptions, Ui, vec2};

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
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                sketch.queue_strip(ui);
                if sketch.drawing_open() {
                    ui.weak("+ open drawing").on_hover_text("Sent with the next message");
                }
                let has_images = sketch.queued() > 0 || sketch.drawing_open();
                let can_send = !self.draft.trim().is_empty() || has_images;
                let draft_id = self.id.with("draft");
                let focused = ui.memory(|m| m.has_focus(draft_id));
                // Plain Enter sends; take it before the text box turns it into a newline.
                let enter = focused && ui.input_mut(crate::consume_plain_enter);
                // A rounded box holding the text and a round send button; a dim outline,
                // a little brighter while typing.
                let v = ui.visuals();
                let stroke_color = if focused { v.widgets.inactive.fg_stroke.color.gamma_multiply(0.45) } else { v.widgets.noninteractive.bg_stroke.color };
                let mut click = false;
                let boxed = egui::Frame::new()
                    .fill(v.extreme_bg_color)
                    .stroke(egui::Stroke::new(1.0, stroke_color))
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin { left: 10, right: 4, top: 4, bottom: 4 })
                    .show(ui, |ui| {
                        // Bottom-aligned, so the button stays at the last line as the text grows.
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Max), |ui| {
                            let button = 28.0;
                            let edit = egui::TextEdit::multiline(&mut self.draft)
                                .id(draft_id)
                                // A custom frame replaces `.margin()`, so the padding goes here.
                                .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 6)))
                                .hint_text("Message the agent")
                                .desired_rows(1)
                                .min_size(vec2(0.0, button))
                                .desired_width(ui.available_width() - button - 8.0);
                            ui.add(edit);
                            click = send_button(ui, button, can_send).clicked();
                        });
                    });
                // Clicking the box's padding focuses the text too.
                if boxed.response.interact(Sense::click()).clicked() {
                    ui.memory_mut(|m| m.request_focus(draft_id));
                }
                if (enter || click) && can_send {
                    sent = Some(Outgoing { text: self.draft.trim().to_string(), images: sketch.take_all() });
                    self.draft.clear();
                    // Follow the conversation again (the sent message arrives as a feed item).
                    self.at_end = true;
                    self.scroll_to_end = true;
                }
            });
            ui.add_space(8.0);
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
            // Activity, not a message: a prompt mark, dim, the time on hover.
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new("›").small().monospace().weak());
                ui.label(RichText::new(&f.text).small().monospace().weak()).on_hover_text(&f.time);
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

/// A round button with an up arrow: accent-filled when there's something to send.
fn send_button(ui: &mut Ui, size: f32, enabled: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), if enabled { Sense::click() } else { Sense::hover() });
    let v = ui.visuals();
    let (fill, fg) = if enabled {
        let fill = if resp.hovered() { v.selection.bg_fill.gamma_multiply(1.25) } else { v.selection.bg_fill };
        (fill, Color32::WHITE)
    } else {
        (v.widgets.inactive.bg_fill, v.weak_text_color())
    };
    let p = ui.painter();
    let c = rect.center();
    let r = size / 2.0;
    p.circle_filled(c, r, fill);
    let stroke = egui::Stroke::new(2.0, fg);
    let (top, bottom) = (c - vec2(0.0, r * 0.45), c + vec2(0.0, r * 0.45));
    p.line_segment([bottom, top], stroke);
    p.line_segment([top, top + vec2(-r * 0.38, r * 0.38)], stroke);
    p.line_segment([top, top + vec2(r * 0.38, r * 0.38)], stroke);
    resp.on_hover_text("Send (Enter)")
}
