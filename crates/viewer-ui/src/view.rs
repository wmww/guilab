//! The session view: zoom and pan, input forwarding to the app, the editor canvas over the
//! image, resize handles, cursors and the view's toasts.

use egui::{Color32, CursorIcon, Event, PointerButton, Pos2, Rect, Sense, Vec2, pos2, vec2};
use guilab_editor::{Canvas, Tool, Xf};
use guilab_image::IRect;
use guilab_kit::Theme;

use super::workspace::{Open, open_doc};
use crate::flight::Place;
use super::{Action, DRAG_SLOP, GestureTo, Input, MAX_ZOOM, MIN_ZOOM, NavMode, PerSession, SessionView, VIEW_TOASTS, ViewerUi, WHEEL_PX, Zoom, linux_button};
use crate::{
    input::{Gesture, HostCursor, HostInput},
    sizing::{self, Resizing},
};

impl ViewerUi {
    /// `menu_was_open`: presses this frame only close the menu.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn view(&mut self, ui: &mut egui::Ui, s: &SessionView, p: &mut PerSession, view: Rect, hovered: bool, inputs: Vec<HostInput>, menu_was_open: bool, out: &mut Vec<Action>) {
        let t = Theme::get(ui.ctx());
        let ws = &mut p.ws;
        // An open image has its own zoom and region (its crop); the live app (and a drawing
        // waiting for its screenshot, which looks the same) the session's.
        let doc_view = ws.image_open();
        let live = matches!(ws.open, Open::Live);
        // The editor owns primary-button input over the view; nothing goes to the app.
        let editing = ws.editor.tool != Tool::Interact || !live;
        let (region, content_scale) = match ws.doc().filter(|_| doc_view) {
            Some(d) => (ws.editor.view_region(d.body().canvas, d.body().crop), d.body().scale),
            None => (IRect::of_size([s.size.0, s.size.1]), s.scale as f32),
        };
        let mut z = if doc_view { ws.doc_zoom } else { p.zoom };
        // "Lock size to window" (the live app only): always fit, no zoom or pan.
        let fixed = p.sizing.follow && !doc_view;
        if fixed {
            z.fit = true;
        }
        if self.resizing.as_ref().is_some_and(|(n, _)| n != s.name) {
            self.resizing = None;
        }
        // Handles: on the live app only, not while the agent holds it or following the window.
        let handles = !s.locked && !fixed && live && s.texture.is_some();
        if !handles && let Some((_, r)) = self.resizing.take() {
            z.fit = r.was_fit;
        }
        let ppp = ui.ctx().pixels_per_point();
        // Image pixels in points at zoom 1.
        let px = vec2(region.w as f32, region.h as f32) / ppp;
        if z.fit {
            let cap = (ppp / content_scale).max(1.0);
            let fit = (view.width() / px.x).min(view.height() / px.y).min(cap);
            z.zoom = fit.max(MIN_ZOOM);
            z.pan = Vec2::ZERO;
        }
        // While a handle drags, the image's opposite side stays put.
        if let Some((_, r)) = &self.resizing {
            z.pan = r.place(px * z.zoom).center() - view.center();
        }
        let image = |z: &Zoom| {
            let size = px * z.zoom;
            let min = view.center() + z.pan - size / 2.0;
            // Snap to physical pixels so 100% stays sharp.
            let min = pos2((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
            Rect::from_min_size(min, size)
        };
        let mut img = image(&z);
        let to_session = |img: Rect, p: Pos2| {
            let rel = (p - img.min) / img.size();
            (rel.x as f64 * s.size.0 as f64, rel.y as f64 * s.size.1 as f64)
        };
        let send = |out: &mut Vec<Action>, input: Input| out.push(Action::Input { session: s.name.to_string(), input });
        let blocked = s.blocked();
        let to_app = !blocked && !editing;
        let now = ui.input(|i| i.time);
        // The human tried to use the app while an agent holds it: say so (a toast).
        let mut tried = false;
        let layer = ui.layer_id();
        let ours = |p: Pos2| view.contains(p) && ui.ctx().layer_id_at(p).is_none_or(|l| l == layer);
        let zoom_at = |z: &mut Zoom, img: &mut Rect, p: Pos2, factor: f32| {
            if fixed {
                return;
            }
            let new = (z.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
            let c = view.center();
            z.pan = (p - c) - (p - c - z.pan) * (new / z.zoom);
            z.zoom = new;
            z.fit = false;
            *img = image(z);
        };
        let tool = ws.editor.tool;
        // A press in the view this frame (an image flying in lands at once).
        let mut pressed_view = false;
        for input in inputs {
            let ev = match input {
                HostInput::Egui(ev) => ev,
                HostInput::Key(k) => {
                    // Keys go to the session while egui doesn't want them (see Viewer).
                    if !k.pressed {
                        if let Some(i) = self.held_keys.iter().position(|c| *c == k.code) {
                            self.held_keys.remove(i);
                            send(out, Input::Key { code: k.code, pressed: false });
                        }
                    } else if !blocked && !self.held_keys.contains(&k.code) {
                        if self.held_in.as_deref() != Some(s.name) {
                            out.extend(self.release_all());
                            self.held_in = Some(s.name.to_string());
                        }
                        self.held_keys.push(k.code);
                        send(out, Input::Key { code: k.code, pressed: true });
                    } else if blocked {
                        tried = true;
                    }
                    continue;
                }
                HostInput::Touch(t) => {
                    let (x, y) = to_session(img, t.pos);
                    match t.phase {
                        egui::TouchPhase::Start => {
                            if !to_app {
                                tried |= blocked;
                                continue;
                            }
                            if self.held_in.as_deref() != Some(s.name) {
                                out.extend(self.release_all());
                                self.held_in = Some(s.name.to_string());
                            }
                            self.held_touches.push(t.id);
                            send(out, Input::TouchDown { id: t.id, x, y });
                        }
                        _ if !self.held_touches.contains(&t.id) => {}
                        egui::TouchPhase::Move => send(out, Input::TouchMotion { id: t.id, x, y }),
                        egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                            self.held_touches.retain(|i| *i != t.id);
                            send(out, Input::TouchUp { id: t.id });
                        }
                    }
                    continue;
                }
                HostInput::Gesture(g) => {
                    // Over the app with "Use the app": the whole gesture goes to it. Else
                    // a pinch zooms the view (swipes do nothing).
                    let k = s.size.0 as f64 / img.width() as f64; // session px per point
                    match g {
                        Gesture::PinchBegin { fingers } | Gesture::SwipeBegin { fingers } => {
                            let pinch = matches!(g, Gesture::PinchBegin { .. });
                            let over_img = self.pointer.is_some_and(|p| ours(p) && img.contains(p));
                            tried |= blocked && !editing && over_img;
                            let over_app = to_app && over_img;
                            if over_app {
                                self.gesture = Some((GestureTo::App { pinch }, s.name.to_string()));
                                send(out, if pinch { Input::PinchBegin { fingers } } else { Input::SwipeBegin { fingers } });
                            } else {
                                self.gesture = pinch.then(|| (GestureTo::View { scale: 1.0 }, s.name.to_string()));
                            }
                        }
                        Gesture::PinchUpdate { dx, dy, scale, rotation } => match &mut self.gesture {
                            Some((GestureTo::App { pinch: true }, _)) => send(out, Input::PinchUpdate { dx: dx * k, dy: dy * k, scale, rotation }),
                            Some((GestureTo::View { scale: last }, _)) => {
                                if scale > 0.0
                                    && *last > 0.0
                                    && let Some(p) = self.pointer
                                {
                                    zoom_at(&mut z, &mut img, p, (scale / *last) as f32);
                                }
                                *last = scale;
                            }
                            _ => {}
                        },
                        Gesture::SwipeUpdate { dx, dy } => {
                            if matches!(self.gesture, Some((GestureTo::App { pinch: false }, _))) {
                                send(out, Input::SwipeUpdate { dx: dx * k, dy: dy * k });
                            }
                        }
                        Gesture::PinchEnd { cancelled } | Gesture::SwipeEnd { cancelled } => {
                            if let Some((GestureTo::App { pinch }, session)) = self.gesture.take() {
                                let input = if pinch { Input::PinchEnd { cancelled } } else { Input::SwipeEnd { cancelled } };
                                out.push(Action::Input { session, input });
                            }
                        }
                    }
                    continue;
                }
            };
            match ev {
                Event::PointerMoved(p) | Event::PointerButton { pos: p, .. } => self.pointer = Some(p),
                Event::PointerGone => {
                    self.pointer = None;
                    if self.held_buttons.is_empty() && self.pointer_in.take().is_some() {
                        send(out, Input::Leave);
                    }
                }
                _ => {}
            }
            match ev {
                Event::PointerMoved(p) => {
                    if let Some((_, r)) = &mut self.resizing {
                        r.moved(p);
                        continue;
                    }
                    if let Some(last) = self.panning {
                        z.pan += p - last;
                        z.fit = false;
                        self.panning = Some(p);
                        img = image(&z);
                        continue;
                    }
                    if let Some((start, _)) = self.middle
                        && (p - start).length() > DRAG_SLOP
                        && !fixed
                    {
                        self.middle = None;
                        z.pan += p - start;
                        z.fit = false;
                        self.panning = Some(p);
                        img = image(&z);
                        continue;
                    }
                    let grabbed = !self.held_buttons.is_empty();
                    if grabbed || (to_app && ours(p) && img.contains(p)) {
                        let (x, y) = to_session(img, p);
                        send(out, Input::Motion { x, y });
                        self.pointer_in = Some(s.name.to_string());
                    } else if self.pointer_in.take().is_some() {
                        send(out, Input::Leave);
                    }
                }
                Event::PointerButton { pos, button, pressed, .. } => {
                    let code = linux_button(button);
                    if !pressed {
                        if button == PointerButton::Primary
                            && let Some((_, r)) = self.resizing.take()
                        {
                            out.push(Action::Resize { session: s.name.to_string(), size: Some(r.size), scale: None });
                            z.fit = r.was_fit;
                            continue;
                        }
                        if self.panning.is_some() && (button == PointerButton::Middle || self.held_buttons.is_empty()) {
                            self.panning = None;
                            continue;
                        }
                        if button == PointerButton::Middle
                            && let Some((_, (x, y))) = self.middle.take()
                        {
                            // A middle click that never moved: deliver it now.
                            if to_app {
                                send(out, Input::Motion { x, y });
                                send(out, Input::Button { button: code, pressed: true });
                                send(out, Input::Button { button: code, pressed: false });
                            }
                            continue;
                        }
                        if let Some(i) = self.held_buttons.iter().position(|b| *b == code) {
                            self.held_buttons.remove(i);
                            send(out, Input::Button { button: code, pressed: false });
                        }
                        continue;
                    }
                    if !ours(pos) || menu_was_open || self.panning.is_some() || self.resizing.is_some() {
                        continue;
                    }
                    pressed_view = true;
                    let on_image = img.contains(pos);
                    // The resize handles are widgets of their own (below).
                    if handles && self.held_buttons.is_empty() && sizing::handle_at(&t, img, view, pos).is_some() {
                        continue;
                    }
                    if button == PointerButton::Middle && self.held_buttons.is_empty() {
                        self.middle = Some((pos, to_session(img, pos)));
                        continue;
                    }
                    if editing {
                        // Primary presses are the editor's. Drawing tools do nothing off the
                        // image, so a drag there pans.
                        let free = button != PointerButton::Primary || tool.draws();
                        if !on_image && free && self.held_buttons.is_empty() && !fixed {
                            self.panning = Some(pos);
                        }
                        continue;
                    }
                    // Off the image: pan.
                    if !on_image {
                        if self.held_buttons.is_empty() && !fixed {
                            self.panning = Some(pos);
                        }
                        continue;
                    }
                    if !to_app {
                        tried |= blocked;
                        continue;
                    }
                    self.held_in = Some(s.name.to_string());
                    self.held_buttons.push(code);
                    let (x, y) = to_session(img, pos);
                    send(out, Input::Motion { x, y });
                    send(out, Input::Button { button: code, pressed: true });
                }
                Event::MouseWheel { unit, delta, phase, modifiers } => {
                    let Some(p) = self.pointer else { continue };
                    if !ours(p) {
                        continue;
                    }
                    if modifiers.ctrl || editing || !img.contains(p) {
                        let smooth = unit == egui::MouseWheelUnit::Point;
                        if !modifiers.ctrl && self.nav.resolve(smooth, self.gestures) == NavMode::ScrollPans {
                            let k = match unit {
                                egui::MouseWheelUnit::Point => 1.0,
                                egui::MouseWheelUnit::Line => 40.0,
                                egui::MouseWheelUnit::Page => view.height(),
                            };
                            if delta != Vec2::ZERO && !fixed {
                                z.pan += delta * k;
                                z.fit = false;
                                img = image(&z);
                            }
                            continue;
                        }
                        let notches = match unit {
                            egui::MouseWheelUnit::Point => delta.y / 50.0,
                            egui::MouseWheelUnit::Line => delta.y,
                            egui::MouseWheelUnit::Page => delta.y * 3.0,
                        };
                        if notches != 0.0 {
                            zoom_at(&mut z, &mut img, p, 1.15f32.powf(notches));
                        }
                        continue;
                    }
                    if blocked {
                        tried = true;
                        continue;
                    }
                    let stop = phase == egui::TouchPhase::End;
                    let input = match unit {
                        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => {
                            let m = if unit == egui::MouseWheelUnit::Page { 3.0 } else { 1.0 };
                            let (sx, sy) = (-delta.x as f64 * m, -delta.y as f64 * m);
                            let px = WHEEL_PX * s.scale;
                            Input::Scroll { dx: sx * px, dy: sy * px, steps: Some((sx, sy)), stop }
                        }
                        egui::MouseWheelUnit::Point => {
                            // Content follows the fingers: host pixels -> session pixels.
                            let k = (s.size.0 as f32 / img.width()) as f64;
                            Input::Scroll { dx: -delta.x as f64 * k, dy: -delta.y as f64 * k, steps: None, stop }
                        }
                    };
                    send(out, input);
                }
                _ => {}
            }
        }

        if tried {
            self.blocked_at = Some(now);
            self.blocked_tries += 1;
        }
        // A tool or the lock took over while the pointer was in the app.
        if !to_app && self.held_buttons.is_empty() && self.pointer_in.take().is_some() {
            send(out, Input::Leave);
        }

        // Clamp the pan so some of the image stays in view.
        let keep = 48.0;
        let max = (img.size() + view.size()) / 2.0 - Vec2::splat(keep);
        let clamped = z.pan.clamp(-max.max(Vec2::ZERO), max.max(Vec2::ZERO));
        if clamped != z.pan {
            z.pan = clamped;
            img = image(&z);
        }

        // An image flying into the view isn't painted by the canvas until it lands; a press
        // in the view lands it now (input is never held up by an animation).
        let open_att = ws.open_key().map(|(_, a)| a);
        let mut flying_in = doc_view && open_att.is_some_and(|a| ws.flights.flying_in(a));
        if flying_in && pressed_view {
            ws.flights.land_out();
            flying_in = false;
        }

        let painter = ui.painter_at(view);
        painter.rect_filled(view, 0.0, t.colors.view_bg);
        // The live app; under an open image it fades out as the image flies in (and back in
        // as it flies home).
        let live_alpha = ws.flights.live.value(now);
        let live_img = if doc_view { live_rect(view, &p.zoom, s, ppp) } else { img };
        match s.texture {
            Some(tex) if live_alpha > 0.0 => {
                painter.image(tex, live_img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE.gamma_multiply(live_alpha));
            }
            None if !doc_view => {
                painter.text(view.center(), egui::Align2::CENTER_CENTER, "(no frame yet)", t.font.body.clone(), t.colors.weak);
            }
            _ => {}
        }

        // The editor over the image. Host zoom is physical px per image px; the canvas
        // counts points.
        let zoom_pt = z.zoom / ppp;
        let xf = Xf { zoom: zoom_pt, offset: img.min.to_vec2() - vec2(region.x as f32, region.y as f32) * zoom_pt };
        let session_zoom = if doc_view { p.zoom } else { z };
        let c = Canvas {
            doc: open_doc(&mut ws.open, &mut ws.pending),
            xf,
            clip: view,
            new_doc: ([s.size.0, s.size.1], s.scale as f32),
            visible: !flying_in,
            input: !menu_was_open,
            toasts: egui::Id::new(VIEW_TOASTS),
        };
        let cout = ws.editor.canvas(ui, c);
        if cout.begin {
            ws.begin();
        }
        if cout.abandon {
            ws.abandon();
        }
        ws.record(cout.edits, session_zoom);
        if let Some(r) = cout.refused {
            ws.notes.push(r);
        }
        if cout.pan != Vec2::ZERO && !fixed {
            z.pan += cout.pan;
            z.fit = false;
        }
        ws.wants_keys = cout.wants_keys;
        ws.view_place = ws.doc().filter(|_| doc_view).map(|d| Place { canvas: xf.rect(IRect::of_size(d.body().canvas).to_rect()), clip: xf.rect(region.to_rect()).intersect(view), alpha: 1.0 });

        // Resize handles: widgets registered after the canvas, so they keep their presses.
        let pointer = ui.input(|i| i.pointer.latest_pos());
        let mut hot_handle = self.resizing.as_ref().map(|(_, r)| r.edge);
        if handles {
            for e in sizing::EDGES {
                let Some(rect) = sizing::handle_rect(&t, img, view, e) else { continue };
                let r = ui.interact(rect, egui::Id::new(("resize-handle", e.x, e.y)), Sense::drag());
                if r.is_pointer_button_down_on()
                    && self.resizing.is_none()
                    && self.panning.is_none()
                    && let Some(start) = ui.input(|i| i.pointer.press_origin())
                {
                    self.resizing = Some((s.name.to_string(), Resizing::new(e, start, img, s.size, z.fit)));
                    z.fit = false;
                }
                if hot_handle.is_none() && r.hovered() && hovered && self.panning.is_none() {
                    hot_handle = Some(e);
                }
            }
            sizing::paint_handles(&painter, &t, img, view, hot_handle);
        }
        if let Some((_, r)) = &mut self.resizing {
            painter.rect_stroke(r.target(), 0.0, egui::Stroke::new(t.size.ring, t.colors.accent), egui::StrokeKind::Outside);
            if let Some(p) = pointer {
                guilab_kit::pointer_label(ui.ctx(), egui::Id::new("resize-readout"), view, p, format!("{} × {}", r.size.0, r.size.1));
            }
            if r.due(now) {
                out.push(Action::Resize { session: s.name.to_string(), size: Some(r.size), scale: None });
                r.sent = Some((r.size, now));
            } else if r.sent.is_some_and(|(sz, _)| sz != r.size) {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(sizing::THROTTLE));
            }
        }
        if doc_view {
            ws.doc_zoom = z;
        } else {
            p.zoom = z;
        }
        self.image_rect = Some(img);
        self.touch_area = (!editing).then_some(img.intersect(view));
        self.pixel_ratio = z.zoom;

        // Cursor. Over the image: the app's when input goes to it, else the editor's tool
        // cursor, never the arrow (the app's own cursor is usually an arrow; it would look
        // like input goes through).
        self.app_image = None;
        let over_img = hovered && pointer.is_some_and(|p| img.contains(p));
        let ctx = ui.ctx();
        if let Some(e) = hot_handle {
            ctx.set_cursor_icon(e.cursor());
        } else if self.panning.is_some() {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
        } else if cout.cursor.is_some() {
            // The canvas set it.
        } else if over_img && !editing {
            if blocked {
                ctx.set_cursor_icon(CursorIcon::NotAllowed);
            } else {
                match &s.cursor {
                    HostCursor::Icon(i) => ctx.set_cursor_icon(*i),
                    HostCursor::Image(img) => {
                        ctx.set_cursor_icon(CursorIcon::Default);
                        self.app_image = Some(img.clone());
                    }
                }
            }
        }
    }
}

/// Where the live app shows in `view` at zoom `z` (while an open image has the view).
fn live_rect(view: Rect, z: &Zoom, s: &SessionView, ppp: f32) -> Rect {
    let px = vec2(s.size.0 as f32, s.size.1 as f32) / ppp;
    let zoom = if z.fit { (view.width() / px.x).min(view.height() / px.y).min((ppp / s.scale as f32).max(1.0)).max(MIN_ZOOM) } else { z.zoom };
    let pan = if z.fit { Vec2::ZERO } else { z.pan };
    let size = px * zoom;
    let min = view.center() + pan - size / 2.0;
    Rect::from_min_size(pos2((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp), size)
}
