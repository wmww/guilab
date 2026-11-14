//! The conversation panel: a feed (human messages, agent messages, activity lines) above
//! a message box with the queued drawings. The host pushes [`FeedItem`]s in, sets the
//! agent's [`Status`] while it works, and gets [`Outgoing`] messages back; it decides what
//! happens to them.

use std::{collections::VecDeque, sync::Arc};

use egui::{ColorImage, Id, RichText, Sense, TextureHandle, TextureOptions, Ui, vec2};
use guilab_kit::Theme;

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

/// What the agent is busy with, shown at the bottom of the feed (animated dots, the text,
/// how long it's been at it). None while it waits for the human.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    /// Seconds since it started working.
    pub elapsed: u64,
    /// Seconds since its last sign of activity (a hint shows after [`QUIET_HINT`]).
    pub quiet: u64,
}

/// Quiet for this long (seconds), the status line says so. Long compiles are normal, so
/// it's only a dim hint; the status stays.
pub const QUIET_HINT: u64 = 120;

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
    status: Option<Status>,
}

impl Chat {
    pub fn new(id: impl Into<Id>) -> Chat {
        Chat { id: id.into(), items: VecDeque::new(), draft: String::new(), max_items: 2000, scroll_to_end: true, at_end: true, status: None }
    }

    pub fn push(&mut self, feed: FeedItem) {
        let n = feed.images.len();
        self.items.push_back(Item { feed, thumbs: vec![None; n], full: vec![None; n] });
        self.scroll_to_end |= self.at_end;
        while self.items.len() > self.max_items {
            self.items.pop_front();
        }
    }

    /// Sets the agent's status (call every frame; it's cheap when unchanged).
    pub fn set_status(&mut self, status: Option<Status>) {
        if status.is_some() && self.status.is_none() {
            self.scroll_to_end |= self.at_end;
        }
        self.status = status;
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
        let t = Theme::get(ui.ctx());
        egui::Panel::bottom(self.id.with("input")).show_separator_line(false).frame(egui::Frame::NONE).show(ui, |ui| {
            ui.add_space(t.space.sm + 2.0);
            ui.horizontal(|ui| {
                sketch.queue_strip(ui);
                if sketch.drawing_open() {
                    ui.weak("+ open drawing").on_hover_text("Sent with the next message");
                }
                let has_images = sketch.queued() > 0 || sketch.drawing_open();
                let draft_id = self.id.with("draft");
                let focused = ui.memory(|m| m.has_focus(draft_id));
                // Plain Enter sends; take it before the text box turns it into a newline (and
                // decide after the box took this pass's typing, which may precede the Enter).
                let enter = focused && ui.input_mut(guilab_kit::consume_plain_enter);
                // A rounded box holding the text and a round send button; a dim outline,
                // brighter while typing.
                let mut click = false;
                let (sm, md) = (t.space.sm as i8, (t.space.md + 2.0) as i8);
                let boxed = guilab_kit::field_frame(&t, focused)
                    .inner_margin(egui::Margin { left: md, right: sm, top: sm, bottom: sm })
                    .show(ui, |ui| {
                        // Bottom-aligned, so the button stays at the last line as the text grows.
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Max), |ui| {
                            let button = t.size.send_button;
                            let edit = egui::TextEdit::multiline(&mut self.draft)
                                .id(draft_id)
                                // A custom frame replaces `.margin()`, so the padding goes here.
                                .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, (t.space.sm + 2.0) as i8)))
                                .hint_text("What's next?")
                                .desired_rows(1)
                                .min_size(vec2(0.0, button))
                                .desired_width(ui.available_width() - button - t.space.md);
                            ui.add(edit);
                            let can_send = !self.draft.trim().is_empty() || has_images;
                            click = guilab_kit::send_button(ui, can_send).clicked();
                        });
                    });
                // Clicking the box's padding focuses the text too.
                if boxed.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::Text).clicked() {
                    ui.memory_mut(|m| m.request_focus(draft_id));
                }
                if (enter || click) && (!self.draft.trim().is_empty() || has_images) {
                    sent = Some(Outgoing { text: self.draft.trim().to_string(), images: sketch.take_all() });
                    self.draft.clear();
                    // Follow the conversation again (the sent message arrives as a feed item).
                    self.at_end = true;
                    self.scroll_to_end = true;
                }
            });
            ui.add_space(t.space.md);
        });

        let id = self.id;
        let scroll = std::mem::take(&mut self.scroll_to_end);
        let out = egui::ScrollArea::vertical().id_salt(id.with("feed")).auto_shrink(false).show(ui, |ui| {
            if self.items.is_empty() {
                ui.weak("Messages, and what the agent does, show up here.");
            }
            for item in self.items.iter_mut() {
                show_item(ui, &t, item);
            }
            if let Some(st) = &self.status {
                show_status(ui, &t, st);
            }
            if scroll {
                ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
            }
        });
        // A scroll to the end lands next frame; until then the offset still looks short.
        self.at_end = scroll || out.state.offset.y + out.inner_rect.height() >= out.content_size.y - 8.0;
        sent
    }
}

fn show_item(ui: &mut Ui, t: &Theme, item: &mut Item) {
    let f = &item.feed;
    match f.who {
        Who::Log => {
            // Activity, not a message: a prompt mark, dim, the time on hover.
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = t.space.sm + 2.0;
                ui.label(RichText::new("›").font(t.font.mono_small.clone()).color(t.colors.weak));
                ui.label(RichText::new(&f.text).font(t.font.mono_small.clone()).color(t.colors.weak)).on_hover_text(&f.time);
            });
        }
        Who::Human | Who::Agent => {
            ui.add_space(t.space.xs);
            ui.horizontal_wrapped(|ui| {
                let (name, color) = match f.who {
                    Who::Human => ("You", t.colors.me),
                    _ => ("Agent", t.colors.agent),
                };
                ui.label(RichText::new(name).strong().color(color));
                ui.label(RichText::new(&f.time).font(t.font.small.clone()).color(t.colors.weak));
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
                        let s = vec2(size[0] as f32, size[1] as f32) * (t.size.feed_thumb_h / size[1] as f32);
                        let resp = guilab_kit::thumb_frame(ui, tex.id(), s, Sense::hover());
                        let full = &mut item.full[i];
                        resp.on_hover_ui(|ui| {
                            let tex = full.get_or_insert_with(|| ui.ctx().load_texture("feed-image", (**img).clone(), TextureOptions::LINEAR));
                            guilab_kit::preview(ui, tex.id(), vec2(img.size[0] as f32, img.size[1] as f32));
                        });
                    }
                });
            }
            ui.add_space(t.space.xs);
        }
    }
}

/// "1m 05s"-style duration.
fn duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        s => format!("{}h {:02}m", s / 3600, s / 60 % 60),
    }
}

/// The agent's status line: three pulsing dots, the status, the time it's been working.
fn show_status(ui: &mut Ui, t: &Theme, st: &Status) {
    ui.add_space(t.space.sm);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = t.space.md;
        let (rect, _) = ui.allocate_exact_size(vec2(26.0, 14.0), Sense::hover());
        let now = ui.input(|i| i.time);
        for i in 0..3 {
            // A wave passing left to right, about once a second.
            let phase = (now * 1.2 - i as f64 * 0.18).rem_euclid(1.0);
            let k = (1.0 - (phase * 2.0 - 0.5).abs().min(1.0) * 2.0).max(0.0) as f32;
            let c = egui::pos2(rect.left() + 4.0 + i as f32 * 9.0, rect.center().y - k * 2.0);
            ui.painter().circle_filled(c, 3.0, t.colors.agent.gamma_multiply(0.35 + 0.65 * k));
        }
        ui.label(RichText::new(&st.text).color(t.colors.agent));
        ui.label(RichText::new(duration(st.elapsed)).font(t.font.small.clone()).color(t.colors.weak)).on_hover_text("Time the agent has been working");
        if st.quiet >= QUIET_HINT {
            ui.label(RichText::new(format!("no guilab activity for {}m", st.quiet / 60)).font(t.font.small.clone()).color(t.colors.weak).italics());
        }
    });
    ui.add_space(t.space.xs);
    // Stepped, not every frame: enough for the dots and the clock, cheap while it works for hours.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
}
