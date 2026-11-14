//! Interactive window moves (`xdg_toplevel.move`, X `_NET_WM_MOVERESIZE`): a pointer grab
//! that drags a floating window, after anvil's `PointerMoveSurfaceGrab`.

use smithay::{
    desktop::Window,
    input::{
        Seat,
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData,
            MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
        },
    },
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::{Logical, Point, Serial},
};

use crate::{session::Session, shell::is_tiled};

pub struct MoveGrab {
    start: GrabStartData<Session>,
    window: Window,
    from: Point<i32, Logical>,
}

impl Session {
    /// Starts moving `w` with the pointer, if the pointer's current grab (the click that
    /// asked for the move, `serial`) is on `surface`'s client. Tiled windows don't move.
    pub(crate) fn start_move(&mut self, w: &Window, surface: &WlSurface, serial: Option<Serial>) {
        if is_tiled(w) {
            return;
        }
        let seat: Seat<Session> = self.seat.clone();
        let Some(ptr) = seat.get_pointer() else { return };
        if serial.is_some_and(|s| !ptr.has_grab(s)) || !ptr.is_grabbed() {
            return;
        }
        let Some(start) = ptr.grab_start_data() else { return };
        if !start.focus.as_ref().is_some_and(|(f, _)| f.id().same_client_as(&surface.id())) {
            return;
        }
        let Some(from) = self.space.element_location(w) else { return };
        let grab = MoveGrab { start, window: w.clone(), from };
        ptr.set_grab(self, grab, serial.unwrap_or_else(|| smithay::utils::SERIAL_COUNTER.next_serial()), smithay::input::pointer::Focus::Clear);
    }
}

impl PointerGrab<Session> for MoveGrab {
    fn motion(&mut self, s: &mut Session, handle: &mut PointerInnerHandle<'_, Session>, _: Option<(WlSurface, Point<f64, Logical>)>, event: &MotionEvent) {
        // No client has pointer focus while moving.
        handle.motion(s, None, event);
        let loc = (self.from.to_f64() + (event.location - self.start.location)).to_i32_round();
        s.drag_window(&self.window, loc);
    }
    fn relative_motion(&mut self, s: &mut Session, handle: &mut PointerInnerHandle<'_, Session>, focus: Option<(WlSurface, Point<f64, Logical>)>, event: &RelativeMotionEvent) {
        handle.relative_motion(s, focus, event);
    }
    fn button(&mut self, s: &mut Session, handle: &mut PointerInnerHandle<'_, Session>, event: &ButtonEvent) {
        handle.button(s, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, s, event.serial, event.time, true);
        }
    }
    fn axis(&mut self, s: &mut Session, handle: &mut PointerInnerHandle<'_, Session>, details: AxisFrame) {
        handle.axis(s, details)
    }
    fn frame(&mut self, s: &mut Session, handle: &mut PointerInnerHandle<'_, Session>) {
        handle.frame(s);
    }
    fn gesture_swipe_begin(&mut self, s: &mut Session, handle: &mut PointerInnerHandle<'_, Session>, event: &GestureSwipeBeginEvent) {
        handle.gesture_swipe_begin(s, event);
    }
    fn gesture_swipe_update(&mut self, s: &mut Session, handle: &mut PointerInnerHandle<'_, Session>, event: &GestureSwipeUpdateEvent) {
        handle.gesture_swipe_update(s, event);
    }
    fn gesture_swipe_end(&mut self, s: &mut Session, handle: &mut PointerInnerHandle<'_, Session>, event: &GestureSwipeEndEvent) {
        handle.gesture_swipe_end(s, event);
    }
    fn gesture_pinch_begin(&mut self, s: &mut Session, handle: &mut PointerInnerHandle<'_, Session>, event: &GesturePinchBeginEvent) {
        handle.gesture_pinch_begin(s, event);
    }
    fn gesture_pinch_update(&mut self, s: &mut Session, handle: &mut PointerInnerHandle<'_, Session>, event: &GesturePinchUpdateEvent) {
        handle.gesture_pinch_update(s, event);
    }
    fn gesture_pinch_end(&mut self, s: &mut Session, handle: &mut PointerInnerHandle<'_, Session>, event: &GesturePinchEndEvent) {
        handle.gesture_pinch_end(s, event);
    }
    fn gesture_hold_begin(&mut self, s: &mut Session, handle: &mut PointerInnerHandle<'_, Session>, event: &GestureHoldBeginEvent) {
        handle.gesture_hold_begin(s, event);
    }
    fn gesture_hold_end(&mut self, s: &mut Session, handle: &mut PointerInnerHandle<'_, Session>, event: &GestureHoldEndEvent) {
        handle.gesture_hold_end(s, event);
    }
    fn start_data(&self) -> &GrabStartData<Session> {
        &self.start
    }
    fn unset(&mut self, _: &mut Session) {}
}
