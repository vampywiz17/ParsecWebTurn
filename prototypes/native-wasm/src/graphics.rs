//! Snapshot-specific GLES import adapter to a real native driver context.
//! No rasterization, framebuffer copy or browser is used for presentation.
use crate::{memory::GuestMemory, window::Window};
use anyhow::{bail, Context, Result};
use glow::HasContext;
use serde::Serialize;
use std::{collections::BTreeMap, ffi::CString, sync::Arc};
use windows_sys::Win32::{
    Foundation::FreeLibrary,
    Graphics::{Gdi::*, OpenGL::*},
    System::LibraryLoader::*,
};

#[derive(Clone, Default, Serialize)]
pub struct GraphicsReport {
    pub api: String,
    pub vendor: String,
    pub renderer: String,
    pub version: String,
    pub accelerated_pixel_format: bool,
    pub frames_presented: u64,
    pub draw_calls: u64,
    pub shaders_compiled: u32,
}

#[derive(Clone, Copy)]
enum Object {
    Buffer(glow::NativeBuffer),
    Texture(glow::NativeTexture),
    Framebuffer(glow::NativeFramebuffer),
    Shader(glow::NativeShader),
    Program(glow::NativeProgram),
    Uniform(Option<glow::NativeUniformLocation>),
}

pub struct Graphics {
    gl: glow::Context,
    dc: HDC,
    context: HGLRC,
    module: windows_sys::Win32::Foundation::HMODULE,
    window: Arc<Window>,
    objects: BTreeMap<u32, Object>,
    next: u32,
    report: GraphicsReport,
    unpack_alignment: usize,
    // A current GL context must never be moved to another OS thread.
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl Graphics {
    pub fn create(window: Arc<Window>) -> Result<Self> {
        unsafe {
            let dc = GetDC(window.handle());
            let temporary = wglCreateContext(dc);
            if temporary.is_null() || wglMakeCurrent(dc, temporary) == 0 {
                if !temporary.is_null() {
                    wglDeleteContext(temporary);
                }
                ReleaseDC(window.handle(), dc);
                bail!("WGL bootstrap failed");
            }
            let modern = wglGetProcAddress(c"wglCreateContextAttribsARB".as_ptr().cast());
            let context = if let Some(proc) = modern.filter(|p| valid_proc(*p as usize)) {
                let create: unsafe extern "system" fn(HDC, HGLRC, *const i32) -> HGLRC =
                    std::mem::transmute(proc);
                // OpenGL 4.1 compatibility includes standardized ES2 shader
                // compatibility, retaining the original #version 100 sources.
                let attrs = [0x2091, 4, 0x2092, 1, 0x9126, 2, 0];
                create(dc, std::ptr::null_mut(), attrs.as_ptr())
            } else {
                std::ptr::null_mut()
            };
            wglMakeCurrent(dc, std::ptr::null_mut());
            wglDeleteContext(temporary);
            if context.is_null() || wglMakeCurrent(dc, context) == 0 {
                if !context.is_null() {
                    wglDeleteContext(context);
                }
                ReleaseDC(window.handle(), dc);
                bail!("Native OpenGL 4.1/ES2 shader compatibility required");
            }
            let module = LoadLibraryW("opengl32.dll\0".encode_utf16().collect::<Vec<_>>().as_ptr());
            if module.is_null() {
                wglMakeCurrent(dc, std::ptr::null_mut());
                wglDeleteContext(context);
                ReleaseDC(window.handle(), dc);
                bail!("Loading the system OpenGL library failed");
            }
            let gl = glow::Context::from_loader_function(|name| {
                let name = CString::new(name).unwrap();
                let p = wglGetProcAddress(name.as_ptr().cast())
                    .map(|p| p as usize)
                    .unwrap_or(0);
                if p > 3 && p != usize::MAX {
                    p as *const _
                } else {
                    GetProcAddress(module, name.as_ptr().cast())
                        .map(|p| p as *const _)
                        .unwrap_or(std::ptr::null())
                }
            });
            if let Some(proc) = wglGetProcAddress(c"wglSwapIntervalEXT".as_ptr().cast())
                .filter(|p| valid_proc(*p as usize))
            {
                let interval: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(proc);
                interval(1);
            }
            let report = GraphicsReport {
                api: "Native OpenGL 4.1 (Matoya UI)".into(),
                vendor: gl.get_parameter_string(glow::VENDOR),
                renderer: gl.get_parameter_string(glow::RENDERER),
                version: gl.get_parameter_string(glow::VERSION),
                accelerated_pixel_format: true,
                ..Default::default()
            };
            let vao = match gl.create_vertex_array() {
                Ok(vao) => vao,
                Err(error) => {
                    wglMakeCurrent(dc, std::ptr::null_mut());
                    wglDeleteContext(context);
                    ReleaseDC(window.handle(), dc);
                    FreeLibrary(module);
                    bail!("GPU vertex array allocation failed: {error}");
                }
            };
            gl.bind_vertex_array(Some(vao));
            *window.graphics.lock().unwrap_or_else(|e| e.into_inner()) = Some(report.clone());
            window
                .active_contexts
                .fetch_add(1, std::sync::atomic::Ordering::Release);
            Ok(Self {
                gl,
                dc,
                context,
                module,
                window,
                objects: BTreeMap::new(),
                next: 1,
                report,
                unpack_alignment: 4,
                _thread: std::marker::PhantomData,
            })
        }
    }

    fn insert(&mut self, obj: Object) -> Result<i32> {
        if self.objects.len() >= 4096 {
            bail!("GPU object limit exceeded");
        }
        let id = self.next;
        self.next = self.next.checked_add(1).context("GPU handle overflow")?;
        self.objects.insert(id, obj);
        Ok(id as i32)
    }
    fn object(&self, id: u32) -> Result<Object> {
        self.objects
            .get(&id)
            .copied()
            .context("invalid GPU object handle")
    }
    fn program(&self, id: u32) -> Result<glow::NativeProgram> {
        if let Object::Program(p) = self.object(id)? {
            Ok(p)
        } else {
            bail!("expected GPU program")
        }
    }
    fn shader(&self, id: u32) -> Result<glow::NativeShader> {
        if let Object::Shader(p) = self.object(id)? {
            Ok(p)
        } else {
            bail!("expected GPU shader")
        }
    }
    fn uniform(&self, id: u32) -> Result<Option<glow::NativeUniformLocation>> {
        if let Object::Uniform(p) = self.object(id)? {
            Ok(p)
        } else {
            bail!("expected GPU uniform")
        }
    }

    pub fn present(&mut self) -> Result<()> {
        // Explicit optional test artifact only: one readback, never the normal
        // presentation path. Capture the rendered backbuffer before swapping.
        if self.report.frames_presented == 10 {
            let capture = self
                .window
                .capture
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            if let Some(path) = capture {
                let (w, h) = *self
                    .window
                    .dimensions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let mut pixels = vec![0; texture_size(w, h, glow::RGBA, glow::UNSIGNED_BYTE)?];
                unsafe {
                    self.gl.read_pixels(
                        0,
                        0,
                        w,
                        h,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelPackData::Slice(Some(&mut pixels)),
                    );
                }
                let image = image::RgbaImage::from_raw(w as u32, h as u32, pixels)
                    .context("Invalid GPU capture size")?;
                image::imageops::flip_vertical(&image).save(path)?;
            }
        }
        unsafe {
            self.gl.flush();
            if SwapBuffers(self.dc) == 0 {
                bail!("native SwapBuffers failed");
            }
        }
        self.report.frames_presented += 1;
        *self
            .window
            .graphics
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(self.report.clone());
        Ok(())
    }

    pub fn call(
        &mut self,
        name: &str,
        m: &GuestMemory,
        args: &[wasmtime::Val],
    ) -> Result<Option<i32>> {
        let i = |n: usize| -> Result<i32> {
            args.get(n)
                .and_then(wasmtime::Val::i32)
                .context("expected GPU i32")
        };
        let u = |n: usize| -> Result<u32> { Ok(i(n)? as u32) };
        let f = |n: usize| -> Result<f32> {
            match args.get(n) {
                Some(wasmtime::Val::F32(bits)) => Ok(f32::from_bits(*bits)),
                _ => bail!("expected GPU f32"),
            }
        };
        unsafe {
            match name {
                "glCreateProgram" => {
                    let p = self.gl.create_program().map_err(anyhow::Error::msg)?;
                    return Ok(Some(self.insert(Object::Program(p))?));
                }
                "glCreateShader" => {
                    let s = self.gl.create_shader(u(0)?).map_err(anyhow::Error::msg)?;
                    return Ok(Some(self.insert(Object::Shader(s))?));
                }
                "glGenBuffers" | "glGenTextures" | "glGenFramebuffers" => {
                    let count = bounded(i(0)?, 256)?;
                    for n in 0..count {
                        let obj = match name {
                            "glGenBuffers" => {
                                Object::Buffer(self.gl.create_buffer().map_err(anyhow::Error::msg)?)
                            }
                            "glGenTextures" => Object::Texture(
                                self.gl.create_texture().map_err(anyhow::Error::msg)?,
                            ),
                            _ => Object::Framebuffer(
                                self.gl.create_framebuffer().map_err(anyhow::Error::msg)?,
                            ),
                        };
                        let id = self.insert(obj)? as u32;
                        m.set_u32(
                            u(1)?
                                .checked_add(n as u32 * 4)
                                .context("GPU handle output overflow")?,
                            id,
                        )?;
                    }
                }
                "glDeleteBuffers" | "glDeleteTextures" | "glDeleteFramebuffers" => {
                    for n in 0..bounded(i(0)?, 256)? {
                        let id = m.u32(
                            u(1)?
                                .checked_add(n as u32 * 4)
                                .context("GPU handle input overflow")?,
                        )?;
                        match self.objects.remove(&id) {
                            Some(Object::Buffer(b)) if name == "glDeleteBuffers" => {
                                self.gl.delete_buffer(b)
                            }
                            Some(Object::Texture(t)) if name == "glDeleteTextures" => {
                                self.gl.delete_texture(t)
                            }
                            Some(Object::Framebuffer(fb)) if name == "glDeleteFramebuffers" => {
                                self.gl.delete_framebuffer(fb)
                            }
                            None => {}
                            _ => bail!("GPU deletion object type mismatch"),
                        }
                    }
                }
                "glBindBuffer" => {
                    let b = if u(1)? == 0 {
                        None
                    } else if let Object::Buffer(b) = self.object(u(1)?)? {
                        Some(b)
                    } else {
                        bail!("expected GPU buffer")
                    };
                    self.gl.bind_buffer(u(0)?, b);
                }
                "glBindTexture" => {
                    let t = if u(1)? == 0 {
                        None
                    } else if let Object::Texture(t) = self.object(u(1)?)? {
                        Some(t)
                    } else {
                        bail!("expected GPU texture")
                    };
                    self.gl.bind_texture(u(0)?, t);
                }
                "glBindFramebuffer" => {
                    let fb = if u(1)? == 0 {
                        None
                    } else if let Object::Framebuffer(fb) = self.object(u(1)?)? {
                        Some(fb)
                    } else {
                        bail!("expected GPU framebuffer")
                    };
                    self.gl.bind_framebuffer(u(0)?, fb);
                }
                "glFramebufferTexture2D" => {
                    let t = if u(3)? == 0 {
                        None
                    } else if let Object::Texture(t) = self.object(u(3)?)? {
                        Some(t)
                    } else {
                        bail!("expected GPU texture")
                    };
                    self.gl
                        .framebuffer_texture_2d(u(0)?, u(1)?, u(2)?, t, i(4)?);
                }
                "glBufferData" => {
                    let bytes = m.read(u(2)?, bounded(i(1)?, 16 * 1024 * 1024)?)?;
                    self.gl.buffer_data_u8_slice(u(0)?, &bytes, u(3)?);
                }
                "glShaderSource" => {
                    let mut source = String::new();
                    for n in 0..bounded(i(1)?, 32)? {
                        let p = m.u32(
                            u(2)?
                                .checked_add(n as u32 * 4)
                                .context("shader pointer overflow")?,
                        )?;
                        source.push_str(&m.string(p, 65536)?);
                    }
                    self.gl.shader_source(self.shader(u(0)?)?, &source);
                }
                "glCompileShader" => {
                    let s = self.shader(u(0)?)?;
                    self.gl.compile_shader(s);
                    if !self.gl.get_shader_compile_status(s) {
                        bail!(
                            "native shader compilation failed: {}",
                            self.gl.get_shader_info_log(s)
                        );
                    }
                    self.report.shaders_compiled += 1;
                }
                "glAttachShader" => self
                    .gl
                    .attach_shader(self.program(u(0)?)?, self.shader(u(1)?)?),
                "glDetachShader" => self
                    .gl
                    .detach_shader(self.program(u(0)?)?, self.shader(u(1)?)?),
                "glLinkProgram" => {
                    let p = self.program(u(0)?)?;
                    self.gl.link_program(p);
                    if !self.gl.get_program_link_status(p) {
                        bail!(
                            "native program link failed: {}",
                            self.gl.get_program_info_log(p)
                        );
                    }
                }
                "glUseProgram" => self.gl.use_program(if u(0)? == 0 {
                    None
                } else {
                    Some(self.program(u(0)?)?)
                }),
                "glDeleteProgram" => {
                    let p = self.program(u(0)?)?;
                    self.gl.delete_program(p);
                    self.objects.remove(&u(0)?);
                }
                "glDeleteShader" => {
                    let s = self.shader(u(0)?)?;
                    self.gl.delete_shader(s);
                    self.objects.remove(&u(0)?);
                }
                "glGetShaderiv" => {
                    let s = self.shader(u(0)?)?;
                    let v = match u(1)? {
                        glow::COMPILE_STATUS => self.gl.get_shader_compile_status(s) as u32,
                        glow::INFO_LOG_LENGTH => self.gl.get_shader_info_log(s).len() as u32 + 1,
                        _ => bail!("unsupported shader query"),
                    };
                    m.set_u32(u(2)?, v)?;
                }
                "glGetShaderInfoLog" => {
                    let log = self.gl.get_shader_info_log(self.shader(u(0)?)?);
                    let max = bounded(i(1)?, 65536)?;
                    if max > 0 {
                        let copied = &log.as_bytes()[..log.len().min(max - 1)];
                        m.write(u(3)?, copied)?;
                        m.write(
                            u(3)?
                                .checked_add(copied.len() as u32)
                                .context("shader log overflow")?,
                            &[0],
                        )?;
                        if u(2)? != 0 {
                            m.set_u32(u(2)?, copied.len() as u32)?;
                        }
                    }
                }
                "glGetProgramiv" => {
                    let p = self.program(u(0)?)?;
                    let value = match u(1)? {
                        glow::LINK_STATUS => self.gl.get_program_link_status(p) as u32,
                        glow::INFO_LOG_LENGTH => self.gl.get_program_info_log(p).len() as u32 + 1,
                        _ => bail!("unsupported program query"),
                    };
                    m.set_u32(u(2)?, value)?;
                }
                "glGetAttribLocation" => {
                    return Ok(Some(
                        self.gl
                            .get_attrib_location(self.program(u(0)?)?, &m.string(u(1)?, 256)?)
                            .map(|n| n as i32)
                            .unwrap_or(-1),
                    ))
                }
                "glGetUniformLocation" => {
                    let p = self.program(u(0)?)?;
                    let loc = self.gl.get_uniform_location(p, &m.string(u(1)?, 256)?);
                    return Ok(Some(self.insert(Object::Uniform(loc))?));
                }
                "glUniform1i" => self.gl.uniform_1_i32(self.uniform(u(0)?)?.as_ref(), i(1)?),
                "glUniform1f" => self.gl.uniform_1_f32(self.uniform(u(0)?)?.as_ref(), f(1)?),
                "glUniform4i" => {
                    self.gl
                        .uniform_4_i32(self.uniform(u(0)?)?.as_ref(), i(1)?, i(2)?, i(3)?, i(4)?)
                }
                "glUniform4f" => {
                    self.gl
                        .uniform_4_f32(self.uniform(u(0)?)?.as_ref(), f(1)?, f(2)?, f(3)?, f(4)?)
                }
                "glUniformMatrix4fv" => {
                    let bytes = m.read(u(3)?, bounded(i(1)?, 64)? * 64)?;
                    let values: Vec<f32> = bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| f32::from_le_bytes(*b))
                        .collect();
                    self.gl.uniform_matrix_4_f32_slice(
                        self.uniform(u(0)?)?.as_ref(),
                        i(2)? != 0,
                        &values,
                    );
                }
                "glTexImage2D" | "glTexSubImage2D" => {
                    let (w, h, format, typ, pointer) = if name == "glTexImage2D" {
                        (i(3)?, i(4)?, u(6)?, u(7)?, u(8)?)
                    } else {
                        (i(4)?, i(5)?, u(6)?, u(7)?, u(8)?)
                    };
                    let size = upload_size(w, h, format, typ, self.unpack_alignment)?;
                    let bytes = if pointer != 0 {
                        Some(m.read(pointer, size)?)
                    } else {
                        None
                    };
                    let pixels = glow::PixelUnpackData::Slice(bytes.as_deref());
                    if name == "glTexImage2D" {
                        self.gl
                            .tex_image_2d(u(0)?, i(1)?, i(2)?, w, h, i(5)?, format, typ, pixels);
                    } else {
                        self.gl.tex_sub_image_2d(
                            u(0)?,
                            i(1)?,
                            i(2)?,
                            i(3)?,
                            w,
                            h,
                            format,
                            typ,
                            pixels,
                        );
                    }
                }
                "glTexParameteri" => self.gl.tex_parameter_i32(u(0)?, u(1)?, i(2)?),
                "glPixelStorei" => {
                    if u(0)? != glow::UNPACK_ALIGNMENT || !matches!(i(1)?, 1 | 2 | 4 | 8) {
                        bail!("Unsupported native UI pixel storage");
                    }
                    self.unpack_alignment = i(1)? as usize;
                    self.gl.pixel_store_i32(u(0)?, i(1)?);
                }
                "glVertexAttribPointer" => {
                    self.gl
                        .vertex_attrib_pointer_f32(u(0)?, i(1)?, u(2)?, i(3)? != 0, i(4)?, i(5)?)
                }
                "glEnableVertexAttribArray" => self.gl.enable_vertex_attrib_array(u(0)?),
                "glDrawElements" => {
                    self.gl.draw_elements(u(0)?, i(1)?, u(2)?, i(3)?);
                    self.report.draw_calls += 1;
                }
                "glEnable" => self.gl.enable(u(0)?),
                "glDisable" => self.gl.disable(u(0)?),
                "glViewport" => self.gl.viewport(i(0)?, i(1)?, i(2)?, i(3)?),
                "glScissor" => self.gl.scissor(i(0)?, i(1)?, i(2)?, i(3)?),
                "glClear" => self.gl.clear(u(0)?),
                "glClearColor" => self.gl.clear_color(f(0)?, f(1)?, f(2)?, f(3)?),
                "glBlendFunc" => self.gl.blend_func(u(0)?, u(1)?),
                "glBlendEquation" => self.gl.blend_equation(u(0)?),
                "glActiveTexture" => self.gl.active_texture(u(0)?),
                "glFinish" => self.gl.finish(),
                "web_gl_flush" => self.gl.flush(),
                "glGetError" => return Ok(Some(self.gl.get_error() as i32)),
                _ => bail!("native GPU bridge not implemented: {name}"),
            }
        }
        Ok(None)
    }
}

impl Drop for Graphics {
    fn drop(&mut self) {
        unsafe {
            self.gl.finish();
            wglMakeCurrent(self.dc, std::ptr::null_mut());
            wglDeleteContext(self.context);
            ReleaseDC(self.window.handle(), self.dc);
            FreeLibrary(self.module);
        }
        *self
            .window
            .graphics
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(self.report.clone());
        self.window
            .active_contexts
            .fetch_sub(1, std::sync::atomic::Ordering::Release);
    }
}

fn bounded(value: i32, max: usize) -> Result<usize> {
    let value = usize::try_from(value).context("negative GPU buffer size")?;
    if value > max {
        bail!("GPU buffer limit exceeded");
    }
    Ok(value)
}
fn valid_proc(pointer: usize) -> bool {
    pointer > 3 && pointer != usize::MAX
}
fn texture_size(w: i32, h: i32, format: u32, typ: u32) -> Result<usize> {
    if typ != glow::UNSIGNED_BYTE {
        bail!("unsupported native UI pixel type");
    }
    let channels = match format {
        glow::RGBA => 4,
        glow::RGB => 3,
        glow::RED | glow::LUMINANCE | glow::ALPHA => 1,
        glow::RG | glow::LUMINANCE_ALPHA => 2,
        _ => bail!("unsupported native UI pixel format"),
    };
    let bytes = bounded(w, 4096)?
        .checked_mul(bounded(h, 4096)?)
        .and_then(|n| n.checked_mul(channels))
        .context("GPU texture size overflow")?;
    if bytes > 16 * 1024 * 1024 {
        bail!("GPU texture limit exceeded");
    }
    Ok(bytes)
}

fn upload_size(w: i32, h: i32, format: u32, typ: u32, alignment: usize) -> Result<usize> {
    if !matches!(alignment, 1 | 2 | 4 | 8) {
        bail!("Invalid pixel alignment");
    }
    let row = texture_size(w, 1, format, typ)?;
    let stride = row
        .checked_add(alignment - 1)
        .context("GPU stride overflow")?
        & !(alignment - 1);
    let height = bounded(h, 4096)?;
    let bytes = if height == 0 {
        0
    } else {
        stride
            .checked_mul(height - 1)
            .and_then(|n| n.checked_add(row))
            .context("GPU upload overflow")?
    };
    if bytes > 16 * 1024 * 1024 {
        bail!("GPU texture limit exceeded");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn texture_uploads_are_checked_before_guest_memory_access() {
        assert_eq!(
            super::upload_size(3, 2, glow::RGB, glow::UNSIGNED_BYTE, 4).unwrap(),
            21
        );
        assert_eq!(
            super::upload_size(3, 2, glow::RGB, glow::UNSIGNED_BYTE, 1).unwrap(),
            18
        );
        assert_eq!(
            super::texture_size(32, 16, glow::RGBA, glow::UNSIGNED_BYTE).unwrap(),
            2048
        );
        assert!(super::texture_size(-1, 16, glow::RGBA, glow::UNSIGNED_BYTE).is_err());
        assert!(super::texture_size(4096, 4096, glow::RGBA, glow::UNSIGNED_BYTE).is_err());
    }
}
