//! Frame clock tick, settle detection, screenshots and output changes.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use anyhow::Context;
use smithay::{
    input::pointer::CursorImageStatus,
    backend::renderer::element::default_primary_scanout_output_compare,
    desktop::utils::{
        OutputPresentationFeedback, send_frames_surface_tree, surface_presentation_feedback_flags_from_states, surface_primary_scanout_output,
        update_surface_primary_scanout_output,
    },
    output::{Mode, Scale},
    reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
    wayland::{compositor::send_surface_state, fractional_scale::with_fractional_scale, presentation::Refresh},
};

use smithay::desktop::PopupManager;

use crate::{
    session::Session,
    shell::{has_buffer, has_pending_configure},
};

/// No commits for this long counts as "done drawing" (a blinking cursor still passes).
pub const SETTLE_QUIET: Duration = Duration::from_millis(150);
/// Commits without a quiet gap for this long count as an animation (spinner, video): its
/// screenshots stop waiting for quiet (see [`Settle::Animating`]).
pub const ANIM_DETECT: Duration = Duration::from_secs(1);
/// While animating, how long after the last input a screenshot waits (the app reacting).
pub const ANIM_REACT: Duration = Duration::from_millis(300);
/// A window from the last `run` is expected this long; until then the session is busy.
pub const NEW_APP_WAIT: Duration = Duration::from_secs(3);
/// How long an unanswered configure (not acked, or acked but not committed) keeps a session
/// busy. Bounded because clients may ack without committing when nothing changes for them.
pub const CONFIGURE_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    /// No toplevel has a buffer yet.
    NoWindow,
    Busy,
    /// Done except that something keeps redrawing: as settled as it gets.
    Animating,
    Settled,
}

impl Session {
    /// One frame: render (only if something may have changed), frame callbacks, cleanup.
    /// Driven by the server's timer at the output refresh rate.
    pub fn tick(&mut self) {
        if std::mem::take(&mut self.dirty) || self.dnd_icon.is_some() {
            self.render();
        }
        let now = self.clock.now();
        let output = self.output.clone();
        // Throttle: surfaces that aren't visible still get a callback once a second.
        for w in self.space.elements() {
            w.send_frame(&output, now, Some(Duration::from_secs(1)), surface_primary_scanout_output);
        }
        if let Some(icon) = &self.dnd_icon {
            send_frames_surface_tree(icon, &output, now, None, |_, _| Some(output.clone()));
        }
        // Cursor surfaces too, so animated cursors run.
        if let CursorImageStatus::Surface(c) = &self.cursor_status {
            send_frames_surface_tree(c, &output, now, None, |_, _| Some(output.clone()));
        }
        self.space.refresh();
        self.popups.cleanup();
        self.track_windows();
        self.flush();
    }

    /// Renders the frame (damage-tracked) and sends presentation feedback.
    fn render(&mut self) {
        let gpu = self.gpu.clone();
        let icon = self.dnd_icon.clone();
        let pointer = self.pointer_location();
        let res = self.frame.render(&mut gpu.borrow_mut(), &self.output, &self.space, icon.as_ref().map(|s| (s, pointer)));
        self.last_render = Instant::now();
        let now = self.clock.now();
        let output = self.output.clone();
        match res {
            Ok(r) => {
                for w in self.space.elements() {
                    w.with_surfaces(|surface, data| {
                        update_surface_primary_scanout_output(
                            surface, &output, data, None, &r.states, default_primary_scanout_output_compare,
                        );
                    });
                }
                if r.damaged {
                    self.frame_seq += 1;
                    let mut fb = OutputPresentationFeedback::new(&output);
                    for w in self.space.elements() {
                        w.take_presentation_feedback(&mut fb, surface_primary_scanout_output, |s, _| {
                            surface_presentation_feedback_flags_from_states(s, None, &r.states)
                        });
                    }
                    fb.presented(now, Refresh::fixed(self.refresh_interval()), self.frame_seq, wp_presentation_feedback::Kind::Vsync);
                }
                self.last_states = Some(r.states);
            }
            Err(e) => {
                tracing::warn!("{}: render failed: {e:#}", self.name);
                self.dirty = true;
            }
        }
    }

    pub fn settle_state(&self, now: Instant) -> Settle {
        let mapped: Vec<_> = self.space.elements().filter_map(|w| w.toplevel()).collect();
        // A configured toplevel without a buffer is still drawing its first frame; so is
        // a mapped X window without one.
        let mut drawing = self.unmapped.iter().any(|w| w.toplevel().is_some_and(|t| t.is_initial_configure_sent()));
        let mut x_windows = 0;
        for x in self.space.elements().filter_map(|w| w.x11_surface()).filter(|x| !x.is_override_redirect()) {
            if x.wl_surface().is_some_and(|s| has_buffer(&s)) {
                x_windows += 1;
            } else {
                drawing = true;
            }
        }
        if mapped.is_empty() && x_windows == 0 {
            return Settle::NoWindow;
        }
        // A popup (menu) that hasn't drawn its first frame yet.
        drawing |= mapped.iter().any(|t| PopupManager::popups_for_surface(t.wl_surface()).any(|(p, _)| !has_buffer(p.wl_surface())));
        let acked = now.duration_since(self.last_configure) >= CONFIGURE_WAIT
            || mapped.iter().all(|t| !has_pending_configure(t.wl_surface()));
        // The app from the last `run` hasn't shown a window yet (another one is up).
        let starting = self
            .last_spawn
            .is_some_and(|(t, id)| t > self.last_map && now.duration_since(t) < NEW_APP_WAIT && self.is_running(id));
        if drawing || !acked || starting {
            return Settle::Busy;
        }
        // Input counts as activity: the app hasn't necessarily reacted (committed) yet.
        let since_input = now.duration_since(self.last_input);
        let quiet = now.duration_since(self.last_commit) >= SETTLE_QUIET && since_input >= SETTLE_QUIET;
        let drawn = self.last_render > self.last_commit;
        if quiet && drawn {
            return Settle::Settled;
        }
        let animating = !quiet && now.duration_since(self.busy_since) >= ANIM_DETECT;
        if animating && since_input >= ANIM_REACT && drawn { Settle::Animating } else { Settle::Busy }
    }

    /// The last rendered frame as opaque RGBA8 (clients may leave alpha 0, e.g. GL areas),
    /// with the cursor drawn in if `cursor`: what screenshots show.
    pub fn capture(&mut self, cursor: bool) -> anyhow::Result<(u32, u32, Vec<u8>)> {
        let (w, h, mut rgba) = self.read_rgba()?;
        rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
        if cursor {
            self.draw_cursor(w, h, &mut rgba);
        }
        Ok((w, h, rgba))
    }

    /// The last rendered frame (whole output, no cursor) as tightly packed RGBA8.
    pub fn read_rgba(&mut self) -> anyhow::Result<(u32, u32, Vec<u8>)> {
        let gpu = self.gpu.clone();
        self.frame.read_rgba(&mut gpu.borrow_mut())
    }

    /// Average color of the frame in a square of `radius` pixels around output pixel (x, y).
    pub fn sample_rgb(&mut self, x: f64, y: f64, radius: i32) -> Option<[u8; 3]> {
        let gpu = self.gpu.clone();
        let (x, y) = (x.round() as i32, y.round() as i32);
        let (_, _, px) = self.frame.read_rgba_region(&mut gpu.borrow_mut(), Some((x - radius, y - radius, 2 * radius + 1, 2 * radius + 1))).ok()?;
        let n = (px.len() / 4).max(1) as u64;
        let mut sum = [0u64; 3];
        for p in px.as_chunks::<4>().0 {
            for c in 0..3 {
                sum[c] += p[c] as u64;
            }
        }
        Some(sum.map(|s| (s / n) as u8))
    }

    /// The color under the pointer (see [`Self::sample_rgb`]), resampled at most every
    /// 100 ms and only when the pointer moved or the frame changed.
    pub fn color_under_pointer(&mut self) -> Option<[u8; 3]> {
        let key = (self.viz.end(), self.frame_seq);
        let now = Instant::now();
        if let Some((k, at, c)) = self.under_pointer
            && (k == key || now.duration_since(at) < Duration::from_millis(100))
        {
            return c;
        }
        let s = self.output.current_scale().fractional_scale();
        let p = self.pointer_location();
        let c = self.sample_rgb(p.x * s, p.y * s, 4);
        self.under_pointer = Some((key, now, c));
        c
    }

    pub fn output_info(&self) -> guilab_proto::OutputInfo {
        let size = self.output.current_mode().map(|m| m.size).unwrap_or_default();
        guilab_proto::OutputInfo {
            width: size.w as u32,
            height: size.h as u32,
            scale: self.output.current_scale().fractional_scale(),
        }
    }

    /// Changes output pixel size and/or scale at runtime; windows follow.
    pub fn set_output(&mut self, size: Option<(i32, i32)>, scale: Option<f64>) -> anyhow::Result<()> {
        guilab_proto::check_output(size.map(|(w, h)| (w as i64, h as i64)), scale).map_err(anyhow::Error::msg)?;
        let mode = size.map(|(w, h)| Mode { size: (w, h).into(), refresh: 60_000 });
        if let Some(m) = mode {
            self.output.set_preferred(m);
        }
        self.output.change_current_state(mode, None, scale.map(Scale::Fractional), None);
        // The space caches output geometry; remap to refresh it.
        self.space.map_output(&self.output, (0, 0));

        if scale.is_some() {
            self.x_apply_scale();
            let s = self.output.current_scale();
            let transform = self.output.current_transform();
            for w in self.space.elements().chain(self.unmapped.iter()) {
                w.with_surfaces(|surface, data| {
                    send_surface_state(surface, data, s.integer_scale(), transform);
                    with_fractional_scale(data, |f| f.set_preferred_scale(s.fractional_scale()));
                });
            }
        }
        self.relayout();
        self.flush();
        Ok(())
    }
}

/// Writes opaque RGBA8 pixels as an RGB PNG (via a temp file, so readers never see half).
pub fn write_rgba_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<()> {
    let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let tmp = path.with_extension("png.tmp");
    let f = std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    enc.write_header()?.write_image_data(&rgb)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
