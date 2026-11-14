//! The feed: messages (a header per run of one author, images above the text), compact
//! activity lines, the status line, following the end, and a jump-to-latest button.

use std::hash::Hash;

use egui::{Color32, CursorIcon, Label, RichText, Sense, Stroke, Ui, pos2, vec2};
use guilab_kit::Theme;

use crate::slot::{Look, slot_aspect};
use crate::{Author, Chat, ChatOut, Entry, SlotId, Status, Thumbs};

pub(crate) struct FeedState {
    scroll_to_end: bool,
    /// Scrolled to the end last frame (new items then keep it there).
    at_end: bool,
    /// Items arrived while scrolled up.
    unseen: bool,
    /// Last frame's content and view heights: short content sits at the bottom, next to
    /// the composer (the empty hint in the middle).
    content_h: f32,
    view_h: f32,
}

impl Default for FeedState {
    fn default() -> Self {
        FeedState { scroll_to_end: true, at_end: true, unseen: false, content_h: 0.0, view_h: 0.0 }
    }
}

impl FeedState {
    pub fn pushed(&mut self) {
        if self.at_end {
            self.scroll_to_end = true;
        } else {
            self.unseen = true;
        }
    }

    pub fn jump(&mut self) {
        self.at_end = true;
        self.scroll_to_end = true;
    }
}

pub(crate) fn show<K: Clone + Eq + Hash>(chat: &mut Chat<K>, ui: &mut Ui, t: &Theme, thumbs: &mut dyn Thumbs<K>, out: &mut ChatOut<K>) {
    let id = chat.id;
    let scroll = std::mem::take(&mut chat.feed.scroll_to_end);
    let pad = t.space.lg;
    // While at the end (or asked to jump there), every frame scrolls to the end, so new
    // items, the status line and a growing composer keep it there. A wheel or scroll bar
    // move away in the same frame still wins, and then it stops following.
    let follow = scroll || chat.feed.at_end;
    // Read before the scroll area consumes the wheel.
    let scrolling = ui.input(|i| i.smooth_scroll_delta.y != 0.0 || i.pointer.primary_down());
    let empty = chat.items.is_empty() && chat.status.is_none();
    let room = (chat.feed.view_h - chat.feed.content_h).max(0.0);
    let mut content_h = 0.0;
    let area = egui::ScrollArea::vertical().id_salt(id.with("feed")).auto_shrink(false).show(ui, |ui| {
        ui.add_space(if empty { room / 2.0 } else { room });
        content_h = egui::Frame::NONE.inner_margin(egui::Margin { left: pad as i8, right: pad as i8, top: t.space.md as i8, bottom: t.space.sm as i8 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if empty {
                ui.vertical_centered(|ui| ui.label(RichText::new(&chat.empty_hint).color(t.colors.weak)));
            }
            let mut prev: Option<&Author> = None;
            for e in &chat.items {
                item(ui, t, id, &chat.me_name, e, prev, &mut chat.slots, thumbs, out);
                prev = Some(&e.item.author);
            }
            if let Some(st) = &chat.status {
                status(ui, t, st);
            }
        })
        .response
        .rect
        .height();
        if follow {
            ui.scroll_to_cursor_animation(Some(egui::Align::BOTTOM), egui::style::ScrollAnimation::none());
        }
    });
    out.feed_clip = area.inner_rect;
    if (content_h, area.inner_rect.height()) != (chat.feed.content_h, chat.feed.view_h) {
        (chat.feed.content_h, chat.feed.view_h) = (content_h, area.inner_rect.height());
        ui.ctx().request_repaint();
    }
    let fits = area.content_size.y <= area.inner_rect.height();
    let at_end = fits || area.state.offset.y + area.inner_rect.height() >= area.content_size.y - 1.0;
    // A follow can land short when the view shrank this frame (egui aims with the old clip
    // size): keep following unless the human scrolled (wheel, or a press on the feed).
    let user = scrolling && ui.input(|i| i.pointer.hover_pos().is_some_and(|p| area.inner_rect.contains(p)));
    chat.feed.at_end = at_end || (follow && !user);
    if !at_end && chat.feed.at_end {
        ui.ctx().request_repaint();
    }
    if chat.feed.at_end {
        chat.feed.unseen = false;
    } else if jump_button(ui, t, id, area.inner_rect, chat.feed.unseen) {
        chat.feed.jump();
        ui.ctx().request_repaint();
    }
}

#[allow(clippy::too_many_arguments)]
fn item<K: Clone + Eq + Hash>(
    ui: &mut Ui,
    t: &Theme,
    chat: egui::Id,
    me: &str,
    e: &Entry<K>,
    prev: Option<&Author>,
    slots: &mut crate::slot::Slots<K>,
    thumbs: &mut dyn Thumbs<K>,
    out: &mut ChatOut<K>,
) {
    let it = &e.item;
    let c = &t.colors;
    match &it.author {
        Author::Activity => {
            ui.add_space(if matches!(prev, Some(Author::Activity) | None) { t.space.xs } else { t.space.lg });
            let font = t.font.mono_small.clone();
            let mark_w = ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = t.space.sm + 2.0;
                // Rows as tall as the text, not a widget: a run of commands stays compact.
                ui.spacing_mut().interact_size.y = 0.0;
                let m = ui.label(RichText::new("›").font(font.clone()).color(c.weak)).rect.width() + t.space.sm + 2.0;
                let hover = if it.time.is_empty() { it.text.clone() } else { format!("{}  {}", it.time, it.text) };
                ui.add(Label::new(RichText::new(&it.text).font(font).color(c.weak)).truncate()).on_hover_text(hover);
                m
            });
            if !it.attachments.is_empty() {
                ui.add_space(t.space.xs);
                ui.horizontal(|ui| {
                    ui.add_space(mark_w.inner);
                    images(ui, t, chat, e, slots, thumbs, out);
                });
            }
        }
        author => {
            let new_run = prev != Some(author);
            if new_run {
                ui.add_space(if prev.is_none() { t.space.xs } else { t.space.lg });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = t.space.md;
                    let (name, color) = match author {
                        Author::Me => (me, c.me),
                        Author::Them { name } => (name.as_str(), c.agent),
                        Author::Activity => unreachable!(),
                    };
                    ui.label(RichText::new(name).strong().color(color));
                    ui.label(RichText::new(&it.time).font(t.font.small.clone()).color(c.weak));
                });
            } else {
                ui.add_space(t.space.sm);
            }
            if !it.attachments.is_empty() {
                ui.add_space(t.space.xs);
                images(ui, t, chat, e, slots, thumbs, out);
            }
            if !it.text.is_empty() {
                if !it.attachments.is_empty() {
                    ui.add_space(t.space.xs);
                }
                let resp = ui.add(Label::new(RichText::new(&it.text).color(c.text)).selectable(true).wrap());
                if !new_run && !it.time.is_empty() {
                    resp.on_hover_text(&it.time);
                }
            }
        }
    }
}

/// An item's images in a row (wrapping).
fn images<K: Clone + Eq + Hash>(ui: &mut Ui, t: &Theme, chat: egui::Id, e: &Entry<K>, slots: &mut crate::slot::Slots<K>, thumbs: &mut dyn Thumbs<K>, out: &mut ChatOut<K>) {
    let faint = e.item.faint;
    let h = t.size.feed_thumb_h * if faint { t.size.faint_scale } else { 1.0 };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(t.space.md, t.space.md);
        for (i, key) in e.item.attachments.iter().enumerate() {
            let slot = SlotId::Feed { item: e.id, index: i as u16 };
            let aspect = thumbs.aspect(key);
            let (rest, _) = ui.allocate_exact_size(vec2(h * slot_aspect(t, aspect), h), Sense::hover());
            let state = thumbs.state(slot, key);
            let look = Look { rest, aspect, faint, appear: 1.0, removable: false, dying: false };
            let ctx = ui.ctx().clone();
            slots.show(ui, t, chat, slot, key, look, Some(state), |px| thumbs.texture(&ctx, key, px), out);
        }
    });
}

/// "1m 05s"-style duration.
pub(crate) fn duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        s => format!("{}h {:02}m", s / 3600, s / 60 % 60),
    }
}

/// The status line: three dots in a wave, the status, the time it's been working, the note.
fn status(ui: &mut Ui, t: &Theme, st: &Status) {
    let c = &t.colors;
    ui.add_space(t.space.lg);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = t.space.md;
        let (rect, _) = ui.allocate_exact_size(vec2(26.0, ui.text_style_height(&egui::TextStyle::Body)), Sense::hover());
        let now = ui.input(|i| i.time);
        for i in 0..3 {
            // A wave passing left to right, about once a second.
            let phase = (now * 1.2 - i as f64 * 0.18).rem_euclid(1.0);
            let k = (1.0 - (phase * 2.0 - 0.5).abs().min(1.0) * 2.0).max(0.0) as f32;
            let p = pos2(rect.left() + 4.0 + i as f32 * 9.0, rect.center().y - k * 2.0);
            ui.painter().circle_filled(p, 3.0, c.agent.gamma_multiply(0.35 + 0.65 * k));
        }
        ui.label(RichText::new(&st.text).color(c.agent));
        ui.label(RichText::new(duration(st.elapsed)).font(t.font.small.clone()).color(c.weak)).on_hover_text("Time spent working");
        if let Some(note) = &st.note {
            ui.label(RichText::new(note).font(t.font.small.clone()).color(c.weak).italics());
        }
    });
    // Stepped, not every frame: enough for the dots and the clock, cheap while it works for hours.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
}

/// A round button over the feed's bottom right corner; accent when there's something new.
fn jump_button(ui: &mut Ui, t: &Theme, id: egui::Id, clip: egui::Rect, unseen: bool) -> bool {
    let c = &t.colors;
    let s = t.size.jump_button;
    let center = pos2(clip.right() - t.space.lg - s / 2.0 - t.space.md, clip.bottom() - t.space.md - s / 2.0);
    let rect = egui::Rect::from_center_size(center, vec2(s, s));
    let resp = ui.interact(rect, id.with("jump"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Jump to the latest");
    let p = ui.painter();
    let (fill, fg) = if unseen {
        (if resp.hovered() { c.accent_hover } else { c.accent }, c.on_accent)
    } else {
        (if resp.hovered() { c.press_fill } else { c.popup }, c.text)
    };
    p.add(egui::Shadow { offset: [0, 2], blur: 8, spread: 0, color: c.shadow }.as_shape(rect, s / 2.0));
    p.circle(center, s / 2.0, fill, Stroke::new(t.size.line, if unseen { Color32::TRANSPARENT } else { c.popup_stroke }));
    let stroke = Stroke::new(2.0, fg);
    let (top, bottom) = (center - vec2(0.0, s * 0.2), center + vec2(0.0, s * 0.2));
    p.line_segment([top, bottom], stroke);
    p.line_segment([bottom, bottom + vec2(-s * 0.17, -s * 0.17)], stroke);
    p.line_segment([bottom, bottom + vec2(s * 0.17, -s * 0.17)], stroke);
    resp.clicked()
}
