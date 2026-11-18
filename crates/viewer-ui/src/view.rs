//! The session view: zoom and pan, input forwarding to the app, the editor canvas over the
//! image, resize handles, cursors and the view's toasts.

use egui::{Color32, CursorIcon, Event, PointerButton, Pos2, Rect, Sense, Vec2, pos2, vec2};
use guilab_editor::{Canvas, Tool, Xf};
use guilab_image::IRect;
use guilab_kit::Theme;

use super::workspace::{Open, open_doc};
use crate::flight::Place;
use super::{Action, CHAT_PANELS, DRAG_SLOP, GestureTo, Input, MAX_ZOOM, MIN_ZOOM, ZoomMode, NavMode, PerSession, SessionView, VIEW_TOASTS, ViewerUi, WHEEL_PX, Zoom, linux_button};
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
        let now = ui.input(|i| i.time);
        // "Resize to window" (the live app only): the app fills the view, so it fits.
        let following = p.sizing.follow && !doc_view;
        if following {
            z.mode = ZoomMode::Fit;
        }
        // Handles: on the live app with a free zoom (Other), not while the agent holds it.
        let handles = !s.locked && live && s.texture.is_some() && z.mode == ZoomMode::Other;
        if !handles || self.resizing.as_ref().is_some_and(|(n, _)| n != s.name) {
            self.resizing = None;
        }
        let ppp = ui.ctx().pixels_per_point();
        // Image pixels in points at zoom 1.
        let px = vec2(region.w as f32, region.h as f32) / ppp;
        let fit_zoom = (view.width() / px.x).min(view.height() / px.y).min((ppp / content_scale).max(1.0)).max(MIN_ZOOM);
        // The target for the mode. Only 1:1 (on an axis where the image overflows the view)
        // and Other leave the center; nothing the human drags can break that.
        let settle = |z: &mut Zoom| match z.mode {
            ZoomMode::Fit => {
                z.zoom = fit_zoom;
                z.pan = Vec2::ZERO;
            }
            ZoomMode::Actual => {
                z.zoom = 1.0;
                let over = ((px - view.size()) / 2.0).max(Vec2::ZERO);
                z.pan = z.pan.clamp(-over, over);
            }
            ZoomMode::Other => {
                let max = ((px * z.zoom + view.size()) / 2.0 - Vec2::splat(PAN_KEEP)).max(Vec2::ZERO);
                z.pan = z.pan.clamp(-max, max);
            }
        };
        // While a handle drags, the image's opposite side stays put.
        if let Some((_, r)) = &self.resizing {
            z.pan = r.place(px * z.zoom).center() - view.center();
        }
        settle(&mut z);
        if self.resizing.is_some() {
            z.snap();
        }
        // Automatic changes (a new mode, the window or app resizing) ease into view.
        if z.ease(now, t.motion.view) {
            ui.ctx().request_repaint();
        }
        let image = |z: &Zoom| {
            let (zoom, pan) = z.shown();
            let size = px * zoom;
            let min = view.center() + pan - size / 2.0;
            // Snap to physical pixels so 100% stays sharp.
            let min = pos2((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
            Rect::from_min_size(min, size)
        };
        let mut img = image(&z);
        // The human moved the view directly: settled for the mode, shown at once.
        let direct = |z: &mut Zoom, img: &mut Rect| {
            settle(z);
            z.snap();
            *img = image(z);
        };
        let to_session = |img: Rect, p: Pos2| {
            let rel = (p - img.min) / img.size();
            (rel.x as f64 * s.size.0 as f64, rel.y as f64 * s.size.1 as f64)
        };
        let send = |out: &mut Vec<Action>, input: Input| out.push(Action::Input { session: s.name.to_string(), input });
        let blocked = s.blocked();
        let to_app = !blocked && !editing;
        // The human tried to use the app while an agent holds it: say so (a toast).
        let mut tried = false;
        let layer = ui.layer_id();
        let ours = |p: Pos2| view.contains(p) && ui.ctx().layer_id_at(p).is_none_or(|l| l == layer);
        // Zooming (wheel, pinch) takes over from any mode: Other, from what's on screen.
        let zoom_at = |z: &mut Zoom, img: &mut Rect, p: Pos2, factor: f32| {
            z.take_over();
            let new = (z.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
            let c = view.center();
            z.pan = (p - c) - (p - c - z.pan) * (new / z.zoom);
            z.zoom = new;
            direct(z, img);
        };
        // Where scrolling and touchpad gestures at the pointer go: the one rule for all of them.
        // With "Use the app" they're the app's over it (held back, not redirected, while the
        // agent has it) and nothing's elsewhere; with a tool or an open image they move the
        // view. Ctrl+scroll always zooms the view.
        let route = |p: Option<Pos2>, ctrl: bool, img: Rect| match p.filter(|p| ours(*p)) {
            None => Route::Nothing,
            Some(_) if editing || ctrl => Route::View,
            Some(p) if !img.contains(p) => Route::Nothing,
            Some(_) if blocked => Route::Blocked,
            Some(_) => Route::App,
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
                    // The whole gesture goes where it began (`route`); in the view a pinch
                    // zooms and a swipe does nothing.
                    let k = s.size.0 as f64 / img.width() as f64; // session px per point
                    match g {
                        Gesture::PinchBegin { fingers } | Gesture::SwipeBegin { fingers } => {
                            let pinch = matches!(g, Gesture::PinchBegin { .. });
                            self.gesture = match route(self.pointer, false, img) {
                                Route::App => {
                                    send(out, if pinch { Input::PinchBegin { fingers } } else { Input::SwipeBegin { fingers } });
                                    Some((GestureTo::App { pinch }, s.name.to_string()))
                                }
                                Route::View => pinch.then(|| (GestureTo::View { scale: 1.0 }, s.name.to_string())),
                                Route::Blocked => {
                                    tried = true;
                                    None
                                }
                                Route::Nothing => None,
                            };
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
                        z.absorb();
                        z.pan += p - last;
                        direct(&mut z, &mut img);
                        self.panning = Some(p);
                        continue;
                    }
                    if let Some((start, _)) = self.middle
                        && (p - start).length() > DRAG_SLOP
                    {
                        self.middle = None;
                        z.absorb();
                        z.pan += p - start;
                        direct(&mut z, &mut img);
                        self.panning = Some(p);
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
                    // The chat divider's grab zone overlaps the view; a press there is egui's.
                    let on_divider = ui.ctx().interaction_snapshot(|i| CHAT_PANELS.iter().any(|id| i.contains_pointer.contains(&egui::Id::new(id).with("__resize"))));
                    if !ours(pos) || on_divider || menu_was_open || self.panning.is_some() || self.resizing.is_some() {
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
                        if !on_image && free && self.held_buttons.is_empty() {
                            self.panning = Some(pos);
                        }
                        continue;
                    }
                    // Off the image: pan.
                    if !on_image {
                        if self.held_buttons.is_empty() {
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
                    let r = route(Some(p), modifiers.ctrl, img);
                    if r == Route::View {
                        let smooth = unit == egui::MouseWheelUnit::Point;
                        if !modifiers.ctrl && self.nav.resolve(smooth, self.gestures) == NavMode::ScrollPans {
                            let k = match unit {
                                egui::MouseWheelUnit::Point => 1.0,
                                egui::MouseWheelUnit::Line => 40.0,
                                egui::MouseWheelUnit::Page => view.height(),
                            };
                            // Scroll-panning takes over from any mode, like zooming.
                            if delta != Vec2::ZERO {
                                z.take_over();
                                z.pan += delta * k;
                                direct(&mut z, &mut img);
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
                    if r != Route::App {
                        tried |= r == Route::Blocked;
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

        // Zooming or scroll-panning out of "Resize to window" keeps the app's current size.
        if following && z.mode != ZoomMode::Fit {
            p.sizing.follow = false;
            p.sizing.before_follow = None;
        }
        // The window or the app may have changed size: settle again (easing).
        settle(&mut z);
        if z.ease(now, t.motion.view) {
            ui.ctx().request_repaint();
        }
        img = image(&z);

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
        let zoom_pt = z.shown().0 / ppp;
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
        if cout.pan != Vec2::ZERO {
            z.absorb();
            z.pan += cout.pan;
            settle(&mut z);
            z.snap();
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
                    self.resizing = Some((s.name.to_string(), Resizing::new(e, start, img, s.size)));
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
        self.pixel_ratio = z.shown().0;

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

/// Points of the image kept in the view however far it's panned.
const PAN_KEEP: f32 = 48.0;

/// Where the live app shows in `view` at zoom `z` (while an open image has the view).
fn live_rect(view: Rect, z: &Zoom, s: &SessionView, ppp: f32) -> Rect {
    let px = vec2(s.size.0 as f32, s.size.1 as f32) / ppp;
    let fit = z.mode == ZoomMode::Fit;
    let zoom = if fit { (view.width() / px.x).min(view.height() / px.y).min((ppp / s.scale as f32).max(1.0)).max(MIN_ZOOM) } else { z.shown().0 };
    let pan = if fit { Vec2::ZERO } else { z.shown().1 };
    let size = px * zoom;
    let min = view.center() + pan - size / 2.0;
    Rect::from_min_size(pos2((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp), size)
}

/// Where a scroll or touchpad gesture goes (see `route` in `view`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    App,
    /// Over the app while the agent holds it: dropped (and the human told).
    Blocked,
    View,
    Nothing,
}
