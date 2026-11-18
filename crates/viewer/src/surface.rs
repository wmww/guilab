//! Buffers for one host surface (the window or a popup): rendered by the shared
//! `GlesRenderer` into dmabufs the host reads directly, or copied into shm.

use std::os::fd::AsFd;

use anyhow::anyhow;
use sctk::{
    dmabuf::DmabufState,
    shm::{Shm, slot::{Buffer as ShmBuffer, SlotPool}},
};
use smithay::{
    backend::{
        allocator::{
            Buffer as _, Fourcc, Modifier, Slot, Swapchain,
            dmabuf::{AsDmabuf, Dmabuf},
            gbm::{GbmAllocator, GbmBuffer, GbmBufferFlags, GbmDevice},
        },
        drm::DrmDeviceFd,
        renderer::{Bind, ExportMem, Frame as _, Offscreen, Renderer as _, gles::{GlesRenderer, GlesTexture}},
    },
    utils::{Rectangle, Transform},
};
use wayland_client::{
    QueueHandle,
    protocol::{wl_buffer::WlBuffer, wl_shm, wl_surface::WlSurface},
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1;

use crate::host::HostState;

/// How frames reach the host (decided once per window, see `HostState::ensure_mode`).
#[derive(Clone)]
pub(crate) enum Mode {
    Dmabuf { gbm: GbmDevice<DrmDeviceFd>, mods: Vec<Modifier> },
    Shm,
}

/// What rendering a surface needs from the host state.
pub(crate) struct Env<'a> {
    pub mode: &'a Mode,
    pub dmabuf: &'a DmabufState,
    pub shm: &'a Shm,
    pub qh: &'a QueueHandle<HostState>,
}

/// A host wl_buffer we own; destroyed with its swapchain slot.
struct OwnedBuffer(WlBuffer);
impl Drop for OwnedBuffer {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

enum Present {
    Dmabuf {
        swapchain: Swapchain<GbmAllocator<DrmDeviceFd>>,
        /// Slots the host still reads from, until `wl_buffer.release`.
        busy: Vec<Slot<GbmBuffer>>,
    },
    Shm {
        pool: SlotPool,
        buffers: Vec<ShmBuffer>,
        target: Option<GlesTexture>,
    },
}

#[derive(Default)]
pub(crate) struct SurfaceBuffers {
    present: Option<Present>,
}

impl SurfaceBuffers {
    /// Renders `size` (physical pixels) with `paint` (GL context current, target bound and
    /// cleared to `clear`, premultiplied) and attaches the buffer to `surface`; the caller
    /// sets damage and commits. Returns false if no buffer is free (all with the host).
    pub fn render(&mut self, env: Env, renderer: &mut GlesRenderer, surface: &WlSurface, (w, h): (u32, u32), clear: [f32; 4], paint: impl FnOnce((u32, u32))) -> anyhow::Result<bool> {
        let size = (w as i32, h as i32);
        let present = match &mut self.present {
            Some(p) => p,
            None => self.present.insert(match env.mode {
                Mode::Dmabuf { gbm, mods } => {
                    let alloc = GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING);
                    Present::Dmabuf { swapchain: Swapchain::new(alloc, w, h, Fourcc::Argb8888, mods.clone()), busy: Vec::new() }
                }
                Mode::Shm => Present::Shm { pool: SlotPool::new((w * h * 4) as usize, env.shm).map_err(|e| anyhow!("shm pool: {e}"))?, buffers: Vec::new(), target: None },
            }),
        };
        match present {
            Present::Dmabuf { swapchain, busy } => {
                swapchain.resize(w, h);
                let Some(slot) = swapchain.acquire().map_err(|e| anyhow!("allocating host buffer: {e}"))? else {
                    return Ok(false);
                };
                let mut dmabuf: Dmabuf = slot.export().map_err(|e| anyhow!("exporting dmabuf: {e}"))?;
                {
                    let mut fb = renderer.bind(&mut dmabuf)?;
                    paint_target(renderer, &mut fb, size, clear, paint)?;
                }
                if slot.userdata().get::<OwnedBuffer>().is_none() {
                    let params = env.dmabuf.create_params(env.qh).map_err(|e| anyhow!("dmabuf params: {e}"))?;
                    let modifier = u64::from(dmabuf.format().modifier);
                    for (i, (fd, (offset, stride))) in dmabuf.handles().zip(dmabuf.offsets().zip(dmabuf.strides())).enumerate() {
                        params.add(fd.as_fd(), i as u32, offset, stride, modifier);
                    }
                    let (buf, params) = params.create_immed(size.0, size.1, Fourcc::Argb8888 as u32, zwp_linux_buffer_params_v1::Flags::empty(), env.qh);
                    params.destroy();
                    slot.userdata().insert_if_missing(|| OwnedBuffer(buf));
                }
                surface.attach(Some(&slot.userdata().get::<OwnedBuffer>().unwrap().0), 0, 0);
                swapchain.submitted(&slot);
                busy.push(slot);
            }
            Present::Shm { pool, buffers, target } => {
                if target.as_ref().is_none_or(|t| smithay::backend::renderer::Texture::size(t) != size.into()) {
                    *target = Some(renderer.create_buffer(Fourcc::Abgr8888, size.into())?);
                    buffers.clear();
                }
                let stride = size.0 * 4;
                let idx = match buffers.iter().position(|b| b.canvas(pool).is_some()) {
                    Some(i) => i,
                    None if buffers.len() < 3 => {
                        buffers.push(pool.create_buffer(size.0, size.1, stride, wl_shm::Format::Argb8888)?.0);
                        buffers.len() - 1
                    }
                    None => return Ok(false),
                };
                let tex = target.as_mut().unwrap();
                let pixels = {
                    let mut fb = renderer.bind(tex)?;
                    paint_target(renderer, &mut fb, size, clear, paint)?;
                    let map = renderer.copy_framebuffer(&fb, Rectangle::from_size(size.into()), Fourcc::Abgr8888)?;
                    drop(fb);
                    renderer.map_texture(&map)?.to_vec()
                };
                let canvas = buffers[idx].canvas(pool).unwrap();
                // Premultiplied RGBA -> little-endian ARGB (BGRA bytes).
                for (d, s) in canvas.as_chunks_mut::<4>().0.iter_mut().zip(pixels.as_chunks::<4>().0) {
                    d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
                }
                buffers[idx].attach_to(surface)?;
            }
        }
        Ok(true)
    }

    /// The host is done reading `buffer`; true if it was one of ours.
    pub fn release(&mut self, buffer: &WlBuffer) -> bool {
        let Some(Present::Dmabuf { busy, .. }) = &mut self.present else { return false };
        let Some(i) = busy.iter().position(|s| s.userdata().get::<OwnedBuffer>().is_some_and(|b| &b.0 == buffer)) else { return false };
        busy.remove(i);
        true
    }
}

/// Clears the bound target and runs `paint` inside a smithay frame (GL context current,
/// target bound), then waits for the GPU.
fn paint_target(renderer: &mut GlesRenderer, fb: &mut smithay::backend::renderer::gles::GlesTarget<'_>, size: (i32, i32), clear: [f32; 4], paint: impl FnOnce((u32, u32))) -> anyhow::Result<()> {
    let mut frame = renderer.render(fb, size.into(), Transform::Normal)?;
    frame.clear(clear.into(), &[Rectangle::from_size(size.into())])?;
    frame.with_context(|_| paint((size.0 as u32, size.1 as u32)))?;
    frame.finish()?.wait().ok();
    Ok(())
}
