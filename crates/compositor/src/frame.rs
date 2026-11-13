//! Frame clock tick, settle detection, screenshots and output changes.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use smithay::{
    backend::renderer::element::default_primary_scanout_output_compare,
    desktop::utils::{
        OutputPresentationFeedback, surface_presentation_feedback_flags_from_states, surface_primary_scanout_output,
        update_surface_primary_scanout_output,
    },
    output::{Mode, Scale},
    reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
    wayland::{compositor::send_surface_state, fractional_scale::with_fractional_scale, presentation::Refresh},
};

use crate::{
    session::Session,
    shell::has_pending_configure,
};

/// No commits for this long counts as "done drawing" (a blinking cursor still passes).
pub const SETTLE_QUIET: Duration = Duration::from_millis(150);
/// How long an unanswered configure (not acked, or acked but not committed) keeps a session
/// busy. Bounded because clients may ack without committing when nothing changes for them.
pub const CONFIGURE_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    /// No toplevel has a buffer yet.
    NoWindow,
    Busy,
    Settled,
}

impl Session {
    /// One frame: render, presentation feedback, frame callbacks, cleanup.
    /// Driven by the server's timer at the output refresh rate.
    pub fn tick(&mut self) {
        let gpu = self.gpu.clone();
        let res = self.frame.render(&mut gpu.borrow_mut(), &self.output, &self.space);
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
            Err(e) => tracing::warn!("{}: render failed: {e:#}", self.name),
        }
        // Throttle: surfaces that aren't visible still get a callback once a second.
        for w in self.space.elements() {
            w.send_frame(&output, now, Some(Duration::from_secs(1)), surface_primary_scanout_output);
        }
        self.space.refresh();
        self.popups.cleanup();
        self.flush();
    }

    pub fn settle_state(&self, now: Instant) -> Settle {
        let mapped: Vec<_> = self.space.elements().filter_map(|w| w.toplevel()).collect();
        if mapped.is_empty() {
            return Settle::NoWindow;
        }
        // A configured toplevel without a buffer is still drawing its first frame.
        let drawing = self.unmapped.iter().any(|w| w.toplevel().is_some_and(|t| t.is_initial_configure_sent()));
        let acked = now.duration_since(self.last_configure) >= CONFIGURE_WAIT
            || mapped.iter().all(|t| !has_pending_configure(t.wl_surface()));
        let quiet = now.duration_since(self.last_commit) >= SETTLE_QUIET;
        let drawn = self.last_render > self.last_commit;
        if !drawing && acked && quiet && drawn { Settle::Settled } else { Settle::Busy }
    }

    /// Writes the last rendered frame as an RGB PNG. Returns its pixel size.
    pub fn write_png(&mut self, path: &Path) -> anyhow::Result<(u32, u32)> {
        let gpu = self.gpu.clone();
        let (w, h, rgba) = self.frame.read_rgba(&mut gpu.borrow_mut())?;
        let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let tmp = path.with_extension("png.tmp");
        let f = std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        enc.write_header()?.write_image_data(&rgb)?;
        std::fs::rename(&tmp, path)?;
        Ok((w, h))
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
        if let Some((w, h)) = size
            && !(1..=16384).contains(&w) | !(1..=16384).contains(&h)
        {
            bail!("size must be 1x1 to 16384x16384");
        }
        if let Some(s) = scale
            && !(0.25..=8.0).contains(&s)
        {
            bail!("scale must be 0.25 to 8");
        }
        let mode = size.map(|(w, h)| Mode { size: (w, h).into(), refresh: 60_000 });
        if let Some(m) = mode {
            self.output.set_preferred(m);
        }
        self.output.change_current_state(mode, None, scale.map(Scale::Fractional), None);
        // The space caches output geometry; remap to refresh it.
        self.space.map_output(&self.output, (0, 0));

        if scale.is_some() {
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
