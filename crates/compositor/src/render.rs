//! Shared renderer (GLES on a DRM render node, pixman fallback) and per-session offscreen frames.

use std::{fs::OpenOptions, os::fd::OwnedFd, path::PathBuf};

use anyhow::{Context, anyhow};
use smithay::{
    backend::{
        allocator::{
            Fourcc,
            dmabuf::Dmabuf,
            gbm::GbmDevice,
        },
        drm::{DrmDeviceFd, DrmNode},
        egl::{EGLContext, EGLDevice, EGLDisplay},
        renderer::{
            Bind, ExportMem, ImportDma, Offscreen,
            damage::OutputDamageTracker,
            element::{RenderElementStates, surface::WaylandSurfaceRenderElement},
            gles::{GlesRenderer, GlesTexture},
            pixman::PixmanRenderer,
        },
    },
    desktop::{Space, Window, space::render_output},
    output::Output,
    reexports::pixman,
    utils::{DeviceFd, Physical, Rectangle, Size},
};

#[allow(clippy::large_enum_variant)] // one per server
pub enum Renderer {
    Gles(GlesRenderer),
    Pixman(PixmanRenderer),
}

/// The server's one renderer, shared by all sessions (`Rc<RefCell<Gpu>>`).
pub struct Gpu {
    pub renderer: Renderer,
    /// Render node, for linux-dmabuf feedback. None with pixman.
    pub node: Option<DrmNode>,
    /// Human-readable description, e.g. "gles /dev/dri/renderD128".
    pub name: String,
}

impl Gpu {
    /// GLES on the first working render node, else pixman.
    /// `GUILAB_RENDERER=pixman` forces pixman; `GUILAB_RENDERER=/dev/dri/renderD129` picks a node.
    pub fn new() -> anyhow::Result<Gpu> {
        let choice = std::env::var("GUILAB_RENDERER").unwrap_or_default();
        if choice != "pixman" {
            let nodes: Vec<PathBuf> = if choice.starts_with('/') {
                vec![choice.into()]
            } else {
                let mut v: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
                    .map(|d| d.flatten().map(|e| e.path()).collect())
                    .unwrap_or_default();
                v.retain(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("renderD")));
                v.sort();
                v
            };
            for path in nodes {
                match Self::gles(&path) {
                    Ok(gpu) => return Ok(gpu),
                    Err(e) => tracing::warn!("GLES on {} failed: {e:#}", path.display()),
                }
            }
            tracing::warn!("no usable render node, falling back to pixman");
        }
        let r = PixmanRenderer::new().map_err(|e| anyhow!("pixman: {e}"))?;
        Ok(Gpu { renderer: Renderer::Pixman(r), node: None, name: "pixman".into() })
    }

    fn gles(path: &std::path::Path) -> anyhow::Result<Gpu> {
        let f = OpenOptions::new().read(true).write(true).open(path)?;
        let fd = DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(f)));
        let gbm = GbmDevice::new(fd).context("gbm")?;
        let egl = unsafe { EGLDisplay::new(gbm) }.context("egl display")?;
        let node = EGLDevice::device_for_display(&egl).ok().and_then(|d| d.try_get_render_node().ok().flatten());
        let ctx = EGLContext::new(&egl).context("egl context")?;
        let r = unsafe { GlesRenderer::new(ctx) }.context("gles renderer")?;
        Ok(Gpu { renderer: Renderer::Gles(r), node, name: format!("gles {}", path.display()) })
    }

    pub fn import_dmabuf(&mut self, buf: &Dmabuf) -> bool {
        match &mut self.renderer {
            Renderer::Gles(r) => r.import_dmabuf(buf, None).is_ok(),
            Renderer::Pixman(r) => r.import_dmabuf(buf, None).is_ok(),
        }
    }
}

enum Target {
    Gles(GlesTexture),
    Pixman(pixman::Image<'static, 'static>),
}

/// A session's persistent offscreen frame. Kept across ticks so the damage tracker
/// can skip unchanged frames (`age` 1 = same buffer as last time).
pub struct Frame {
    target: Option<(Target, Size<i32, Physical>)>,
    damage: OutputDamageTracker,
    age: usize,
}

pub struct Rendered {
    pub states: RenderElementStates,
    /// Something on screen changed.
    pub damaged: bool,
}

const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

impl Frame {
    pub fn new(output: &Output) -> Self {
        Frame { target: None, damage: OutputDamageTracker::from_output(output), age: 0 }
    }

    fn ensure_target(&mut self, gpu: &mut Gpu, size: Size<i32, Physical>) -> anyhow::Result<()> {
        let matches = matches!(
            (&self.target, &gpu.renderer),
            (Some((Target::Gles(_), s)), Renderer::Gles(_)) | (Some((Target::Pixman(_), s)), Renderer::Pixman(_)) if *s == size
        );
        if matches {
            return Ok(());
        }
        let bsize = (size.w, size.h).into();
        let t = match &mut gpu.renderer {
            Renderer::Gles(r) => Target::Gles(r.create_buffer(Fourcc::Abgr8888, bsize)?),
            Renderer::Pixman(r) => Target::Pixman(r.create_buffer(Fourcc::Abgr8888, bsize)?),
        };
        self.target = Some((t, size));
        self.age = 0;
        Ok(())
    }

    /// Draws the space into the frame.
    pub fn render(&mut self, gpu: &mut Gpu, output: &Output, space: &Space<Window>) -> anyhow::Result<Rendered> {
        let size = output.current_mode().context("output has no mode")?.size;
        self.ensure_target(gpu, size)?;
        let (target, _) = self.target.as_mut().unwrap();
        let age = self.age;
        macro_rules! go {
            ($r:expr, $t:expr, $R:ty) => {{
                let mut fb = $r.bind($t)?;
                let res = render_output::<_, WaylandSurfaceRenderElement<$R>, _, _>(
                    output, $r, &mut fb, 1.0, age, [space], &[], &mut self.damage, CLEAR,
                )
                .map_err(|e| anyhow!("render: {e:?}"))?;
                Rendered { damaged: res.damage.is_some_and(|d| !d.is_empty()), states: res.states }
            }};
        }
        let out = match (&mut gpu.renderer, target) {
            (Renderer::Gles(r), Target::Gles(t)) => go!(r, t, GlesRenderer),
            (Renderer::Pixman(r), Target::Pixman(t)) => go!(r, t, PixmanRenderer),
            _ => unreachable!("target matches renderer"),
        };
        self.age = 1;
        Ok(out)
    }

    /// Reads the last rendered frame back as tightly packed RGBA8.
    pub fn read_rgba(&mut self, gpu: &mut Gpu) -> anyhow::Result<(u32, u32, Vec<u8>)> {
        let (target, size) = self.target.as_mut().context("nothing rendered yet")?;
        let size = *size;
        let rect = Rectangle::from_size((size.w, size.h).into());
        macro_rules! go {
            ($r:expr, $t:expr) => {{
                let fb = $r.bind($t)?;
                let map = $r.copy_framebuffer(&fb, rect, Fourcc::Abgr8888)?;
                drop(fb);
                $r.map_texture(&map)?.to_vec()
            }};
        }
        let px = match (&mut gpu.renderer, target) {
            (Renderer::Gles(r), Target::Gles(t)) => go!(r, t),
            (Renderer::Pixman(r), Target::Pixman(t)) => go!(r, t),
            _ => unreachable!("target matches renderer"),
        };
        Ok((size.w as u32, size.h as u32, px))
    }
}
