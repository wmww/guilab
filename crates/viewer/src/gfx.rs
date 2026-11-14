//! egui on smithay's GLES context: an `egui_glow::Painter` with its own glow bindings
//! (smithay uses an older glow), and "mirrors": egui-owned copies of smithay textures.
//!
//! Why copies: egui_glow deletes every texture registered with it when it's freed or
//! replaced, so registering a texture smithay still renders into would break it. A GPU copy
//! of the visible frame costs little and only happens when that frame changed.

use std::{collections::HashMap, num::NonZeroU32, sync::Arc};

use anyhow::{Context, anyhow};
use glow::HasContext;
use smithay::backend::renderer::gles::GlesRenderer;

struct Mirror {
    tex: glow::Texture,
    id: egui::TextureId,
    size: (i32, i32),
    seq: u64,
    nearest: bool,
}

pub struct Gfx {
    pub ctx: egui::Context,
    painter: egui_glow::Painter,
    gl: Arc<glow::Context>,
    fbos: (glow::Framebuffer, glow::Framebuffer),
    mirrors: HashMap<String, Mirror>,
}

impl Gfx {
    pub fn new(renderer: &mut GlesRenderer) -> anyhow::Result<Gfx> {
        renderer
            .with_context(|_| unsafe {
                let gl = Arc::new(glow::Context::from_loader_function(|s| {
                    let s = std::ffi::CString::new(s).unwrap();
                    smithay::backend::egl::get_proc_address(s.to_str().unwrap()) as *const _
                }));
                let painter = egui_glow::Painter::new(gl.clone(), "", None, false).map_err(|e| anyhow!("egui painter: {e}"))?;
                let fbos = (gl.create_framebuffer().map_err(|e| anyhow!(e))?, gl.create_framebuffer().map_err(|e| anyhow!(e))?);
                let ctx = egui::Context::default();
                // Debug builds would outline unaligned widgets in red; this is a product UI.
                ctx.all_styles_mut(|s| {
                    s.debug.show_unaligned = false;
                    s.debug.warn_if_rect_changes_id = false;
                });
                Ok(Gfx { ctx, painter, gl, fbos, mirrors: HashMap::new() })
            })
            .context("GL context")?
    }

    /// Frees GL resources. Call before dropping.
    pub fn destroy(mut self, renderer: &mut GlesRenderer) {
        let _ = renderer.with_context(|_| unsafe {
            // The painter deletes the mirror textures (registered with it) itself.
            self.gl.delete_framebuffer(self.fbos.0);
            self.gl.delete_framebuffer(self.fbos.1);
            self.painter.destroy();
        });
    }

    /// Makes egui texture `key` a copy of GL texture `src` (`size` pixels) if `seq` changed
    /// since the last copy. Call with the GL context current (`renderer.with_context`).
    pub fn mirror(&mut self, key: &str, src: u32, size: (i32, i32), seq: u64, nearest: bool) -> egui::TextureId {
        let gl = self.gl.clone();
        let m = match self.mirrors.get_mut(key) {
            Some(m) if m.size == size => m,
            _ => unsafe {
                let tex = gl.create_texture().expect("create texture");
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, size.0, size.1);
                let id = match self.mirrors.remove(key) {
                    Some(old) => {
                        self.painter.replace_native_texture(old.id, tex);
                        old.id
                    }
                    None => self.painter.register_native_texture(tex),
                };
                self.mirrors.insert(key.to_string(), Mirror { tex, id, size, seq: seq.wrapping_sub(1), nearest: !nearest });
                self.mirrors.get_mut(key).unwrap()
            },
        };
        unsafe {
            if m.nearest != nearest {
                let f = if nearest { glow::NEAREST } else { glow::LINEAR } as i32;
                gl.bind_texture(glow::TEXTURE_2D, Some(m.tex));
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, f);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, f);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                m.nearest = nearest;
            }
            if m.seq != seq {
                let read = gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING);
                let draw = gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
                let src = glow::NativeTexture(NonZeroU32::new(src).expect("texture id"));
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.fbos.0));
                gl.framebuffer_texture_2d(glow::READ_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(src), 0);
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.fbos.1));
                gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(m.tex), 0);
                gl.disable(glow::SCISSOR_TEST);
                gl.blit_framebuffer(0, 0, size.0, size.1, 0, 0, size.0, size.1, glow::COLOR_BUFFER_BIT, glow::NEAREST);
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, NonZeroU32::new(read as u32).map(glow::NativeFramebuffer));
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, NonZeroU32::new(draw as u32).map(glow::NativeFramebuffer));
                m.seq = seq;
            }
        }
        m.id
    }

    /// Forgets mirrors whose key isn't in `keep`.
    pub fn retain_mirrors(&mut self, keep: impl Fn(&str) -> bool) {
        let gone: Vec<String> = self.mirrors.keys().filter(|k| !keep(k)).cloned().collect();
        for k in gone {
            let m = self.mirrors.remove(&k).unwrap();
            self.painter.free_texture(m.id);
        }
    }

    pub fn max_texture_side(&self) -> usize {
        self.painter.max_texture_side()
    }

    /// Paints egui output into the bound framebuffer. Call inside a smithay frame.
    pub fn paint(&mut self, size: (u32, u32), ppp: f32, prims: &[egui::ClippedPrimitive], delta: &egui::TexturesDelta) {
        self.painter.paint_and_update_textures([size.0, size.1], ppp, prims, delta);
        // Leave GL state as smithay expects it.
        unsafe {
            let gl = &self.gl;
            gl.disable(glow::SCISSOR_TEST);
            gl.use_program(None);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        }
    }
}
