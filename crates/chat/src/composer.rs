//! The composer: a rounded box with the pending attachments in a row above the text, and
//! a send button. Pending slots grow in and shrink out; Enter sends, Shift+Enter is a newline.

use std::hash::Hash;

use egui::{Align, Key, Layout, Modifiers, Rect, Sense, TextureId, Ui, UiBuilder, pos2, vec2};
use guilab_kit::Theme;

use crate::slot::{Look, slot_aspect};
use crate::{Chat, ChatEvent, ChatOut, SlotId, Thumbs};

/// One composer slot, live or shrinking out.
struct CSlot<K> {
    key: K,
    born: f64,
    gone: Option<f64>,
    aspect: f32,
    /// Last texture shown, for shrinking out after the host dropped the key.
    tex: Option<TextureId>,
    /// It was pending when the message was sent: it leaves at once, not shrinking.
    sent: bool,
}

pub(crate) struct ComposerState<K> {
    slots: Vec<CSlot<K>>,
    shown_once: bool,
    focused: bool,
    /// The text changed since it got focus (else undo keys go to the host).
    edited: bool,
}

impl<K> Default for ComposerState<K> {
    fn default() -> Self {
        ComposerState { slots: Vec::new(), shown_once: false, focused: false, edited: false }
    }
}

fn ease(t: &Theme, since: f64, now: f64) -> f32 {
    let m = t.motion.slot_grow;
    if m.secs <= 0.0 { 1.0 } else { m.easing.apply(((now - since) / m.secs as f64) as f32) }
}

impl<K> CSlot<K> {
    fn appear(&self, t: &Theme, now: f64) -> f32 {
        ease(t, self.born, now) * self.gone.map_or(1.0, |g| 1.0 - ease(t, g, now))
    }
}

impl<K: Clone + Eq + Hash> ComposerState<K> {
    /// Matches the slots to `pending`: new keys grow in, missing ones shrink out (or leave
    /// at once after a send), a key coming back (undo) grows back from where it was.
    fn reconcile(&mut self, t: &Theme, pending: &[K], now: f64) {
        let secs = t.motion.slot_grow.secs as f64;
        let first = !self.shown_once;
        self.shown_once = true;
        let old = std::mem::take(&mut self.slots);
        let mut live: Vec<CSlot<K>> = Vec::with_capacity(pending.len());
        // Dying slots, after the live key they followed (None = first).
        let mut dying: Vec<(Option<K>, CSlot<K>)> = Vec::new();
        let mut old_live = Vec::new();
        let mut anchor = None;
        for mut s in old {
            if pending.contains(&s.key) {
                if s.gone.is_some() {
                    // Back: grow from the current size.
                    let v = s.appear(t, now) as f64;
                    s.born = now - v * secs;
                    s.gone = None;
                }
                anchor = Some(s.key.clone());
                old_live.push(s);
            } else if s.sent {
                // Sent: gone without shrinking.
            } else {
                if s.gone.is_none() {
                    s.gone = Some(now);
                }
                if now - s.gone.unwrap_or(now) < secs {
                    dying.push((anchor.clone(), s));
                }
            }
        }
        for k in pending {
            match old_live.iter().position(|s| s.key == *k) {
                Some(i) => live.push(old_live.swap_remove(i)),
                None => live.push(CSlot {
                    key: k.clone(),
                    born: if first { f64::NEG_INFINITY } else { now },
                    gone: None,
                    aspect: 1.0,
                    tex: None,
                    sent: false,
                }),
            }
        }
        let take = |dying: &mut Vec<(Option<K>, CSlot<K>)>, a: Option<&K>, out: &mut Vec<CSlot<K>>| {
            let mut i = 0;
            while i < dying.len() {
                if dying[i].0.as_ref() == a {
                    out.push(dying.remove(i).1);
                } else {
                    i += 1;
                }
            }
        };
        let mut slots = Vec::with_capacity(live.len() + dying.len());
        take(&mut dying, None, &mut slots);
        for s in live {
            let k = s.key.clone();
            slots.push(s);
            take(&mut dying, Some(&k), &mut slots);
        }
        // Anchors that disappeared: keep them at the end.
        slots.extend(dying.drain(..).map(|(_, s)| s));
        // A send's keys are gone by now if the host took them.
        for s in &mut slots {
            s.sent = false;
        }
        self.slots = slots;
    }
}

/// Returns true when a message was sent.
pub(crate) fn show<K: Clone + Eq + Hash>(chat: &mut Chat<K>, ui: &mut Ui, t: &Theme, pending: &[K], thumbs: &mut dyn Thumbs<K>, out: &mut ChatOut<K>) -> bool {
    let now = ui.input(|i| i.time);
    let id = chat.id;
    let text_id = chat.text_id();
    chat.composer.reconcile(t, pending, now);
    let mut sent = false;

    egui::Panel::bottom(id.with("composer")).show_separator_line(false).frame(egui::Frame::NONE.inner_margin(egui::Margin {
        left: t.space.lg as i8,
        right: t.space.lg as i8,
        top: t.space.sm as i8,
        bottom: t.space.lg as i8,
    }))
    .show(ui, |ui| {
        let focused = ui.memory(|m| m.has_focus(text_id));
        let st = &mut chat.composer;
        if focused && !st.focused {
            st.edited = false;
        }
        st.focused = focused;
        // Plain Enter sends: take it before the text box makes it a newline, and decide after
        // the box took this pass's typing (which may come before the Enter).
        let enter = focused && ui.input_mut(guilab_kit::consume_plain_enter);
        // Undo keys with no edits of its own since it got focus are the host's.
        if focused && !st.edited {
            let (redo, undo) = ui.input_mut(|i| {
                let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y);
                (redo, i.consume_key(Modifiers::COMMAND, Key::Z))
            });
            if undo {
                out.events.push(ChatEvent::Undo);
            }
            if redo {
                out.events.push(ChatEvent::Redo);
            }
        }

        let (md, sm) = (t.space.md, t.space.sm);
        let frame = guilab_kit::field_frame(t, focused).inner_margin(egui::Margin { left: md as i8, right: sm as i8, top: sm as i8, bottom: sm as i8 });
        let was_empty = chat.draft.is_empty();
        let mut click = false;
        let mut changed = false;
        let boxed = ui.scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
            let framed = frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                // Attachments, above the text.
                let st = &mut chat.composer;
                let h = t.size.feed_thumb_h;
                let gap = md;
                let row = st.slots.iter().map(|s| s.appear(t, now)).fold(0.0, f32::max);
                if row > 0.0 {
                    ui.add_space(sm * row);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(0.0, gap);
                        for s in &mut st.slots {
                            let a = s.appear(t, now);
                            let live = s.gone.is_none();
                            if live {
                                s.aspect = thumbs.aspect(&s.key);
                            }
                            let w = h * slot_aspect(t, s.aspect);
                            let (alloc, _) = ui.allocate_exact_size(vec2((w + gap) * a, h * row), Sense::hover());
                            let rest = Rect::from_center_size(pos2(alloc.min.x + w * a / 2.0, alloc.min.y + h * row / 2.0), vec2(w, h));
                            let state = live.then(|| thumbs.state(SlotId::Composer, &s.key));
                            let look = Look { rest, aspect: s.aspect, faint: false, appear: a, removable: true, dying: !live };
                            let cached = s.tex;
                            let key = s.key.clone();
                            let ctx = ui.ctx().clone();
                            let tex = chat.slots.show(ui, t, id, SlotId::Composer, &key, look, state, |px| if live { thumbs.texture(&ctx, &key, px) } else { cached }, out);
                            if tex.is_some() {
                                s.tex = tex;
                            }
                        }
                    });
                    ui.add_space(sm * row);
                }
                // The text and the send button at its last line. (A bottom-aligned row would
                // fill the panel's previous height, so the box could never shrink.)
                ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                    let button = t.size.send_button;
                    let w = (ui.available_width() - button - md).max(0.0);
                    let line = ui.text_style_height(&egui::TextStyle::Body);
                    // One line sits centered on the send button.
                    let pad = ((button - line) / 2.0).max(0.0);
                    let max_h = line * t.size.composer_lines + 2.0 * pad;
                    let text = ui.allocate_ui_with_layout(vec2(w, max_h), Layout::top_down(Align::Min), |ui| {
                        egui::ScrollArea::vertical().id_salt(id.with("text-scroll")).max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                            let edit = egui::TextEdit::multiline(&mut chat.draft)
                                .id(text_id)
                                // A custom frame replaces `.margin()`, so the padding goes here.
                                .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, pad as i8)))
                                .hint_text(chat.hint.as_str())
                                .desired_rows(1)
                                .min_size(vec2(0.0, button))
                                .desired_width(w - t.space.md);
                            changed = ui.add(edit).changed();
                        });
                    });
                    let text_h = text.response.rect.height();
                    let can_send = !chat.draft.trim().is_empty() || !pending.is_empty();
                    ui.vertical(|ui| {
                        ui.add_space((text_h - button).max(0.0));
                        let b = guilab_kit::send_button(ui, can_send);
                        let tip = if can_send { &chat.send_hint } else { &chat.send_empty_hint };
                        click = b.on_hover_text(tip.as_str()).clicked();
                    });
                });
            });
            framed.response.rect
        });
        if changed {
            chat.composer.edited = true;
            if was_empty && !chat.draft.is_empty() {
                out.events.push(ChatEvent::Started);
            }
        }
        out.composer = boxed.inner;
        // Clicking the box's padding focuses the text.
        if boxed.response.on_hover_cursor(egui::CursorIcon::Text).clicked() {
            ui.memory_mut(|m| m.request_focus(text_id));
        }
        if (enter || click) && (!chat.draft.trim().is_empty() || !pending.is_empty()) {
            out.events.push(ChatEvent::Send { text: chat.draft.trim().to_string() });
            chat.draft.clear();
            for s in &mut chat.composer.slots {
                s.sent = true;
            }
            chat.composer.edited = false;
            if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), text_id) {
                st.clear_undoer();
                st.store(ui.ctx(), text_id);
            }
            ui.memory_mut(|m| m.request_focus(text_id));
            sent = true;
        }
    });
    if chat.composer.slots.iter().any(|s| s.appear(t, now) < 1.0) {
        ui.ctx().request_repaint();
    }
    sent
}
