//! Mirror only the pinned Matoya GUI's CPU draw commands. No framebuffer readback.
//! Original GL rendering remains authoritative for account/login screens.
use crate::memory::GuestMemory;
use anyhow::{bail, Context, Result};
use std::{collections::BTreeMap, sync::Arc};
#[derive(Clone)]
pub struct Texture {
    pub version: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
#[derive(Clone)]
pub struct Batch {
    pub vertices: Vec<u8>,
    pub texture: Arc<Texture>,
    pub clip: [i32; 4],
}
#[derive(Clone, Default)]
pub struct Frame {
    pub size: (u32, u32),
    pub batches: Vec<Batch>,
}
#[derive(Clone, Default, serde::Serialize)]
pub struct Report {
    pub frames_published: u64,
    pub draw_calls_composited: u64,
    pub failure: Option<&'static str>,
    pub failure_call: Option<String>,
    pub failure_detail: Option<String>,
}
#[derive(Default)]
pub struct Shared {
    pub frame: Arc<Frame>,
    pub report: Report,
}
#[derive(Clone, Copy, Default)]
struct Attribute {
    buffer: u32,
    size: i32,
    kind: u32,
    normalized: bool,
    stride: usize,
    offset: usize,
}
#[derive(Default)]
pub struct Capture {
    buffers: BTreeMap<u32, Vec<u8>>,
    textures: BTreeMap<u32, Arc<Texture>>,
    attributes: BTreeMap<u32, Attribute>,
    semantics: BTreeMap<u32, BTreeMap<String, u32>>,
    uniforms: BTreeMap<u32, u32>,
    sampler_uniforms: BTreeMap<u32, u32>,
    samplers: BTreeMap<u32, u32>,
    bindings: BTreeMap<u32, u32>,
    internal_formats: BTreeMap<u32, u32>,
    unpack_row_length: usize,
    unpack_alignment: usize,
    projections: BTreeMap<u32, [f32; 16]>,
    array: u32,
    elements: u32,
    texture: u32,
    program: u32,
    active_texture: u32,
    viewport: [i32; 4],
    scissor: Option<[i32; 4]>,
    clip: [i32; 4],
    version: u64,
    bytes: usize,
    frame: Frame,
    failed: bool,
    failure_call: Option<String>,
    failure_detail: Option<String>,
}
impl Capture {
    pub fn publish(&mut self, shared: &mut Shared) {
        if self.failed {
            shared.frame = Arc::default();
            shared.report.failure_call = self.failure_call.clone();
            shared.report.failure_detail = self.failure_detail.clone();
            shared.report.failure = Some("unsupported-or-invalid-pinned-gui-command");
            return;
        }
        shared.frame = Arc::new(std::mem::take(&mut self.frame));
        shared.report.frames_published += 1;
    }
    pub fn observe(
        &mut self,
        name: &str,
        m: &GuestMemory,
        args: &[wasmtime::Val],
        result: Option<i32>,
    ) {
        if !self.failed {
            if let Err(error) = self.record(name, m, args, result) {
                self.failed = true;
                self.failure_call = Some(name.into());
                // These errors contain only fixed adapter categories, never UI strings/pixels.
                self.failure_detail = Some(error.to_string());
                self.frame = Frame::default();
            }
        }
    }
    fn record(
        &mut self,
        name: &str,
        m: &GuestMemory,
        args: &[wasmtime::Val],
        result: Option<i32>,
    ) -> Result<()> {
        let i = |n: usize| {
            args.get(n)
                .and_then(wasmtime::Val::i32)
                .context("UI integer missing")
        };
        let u = |n: usize| Ok::<u32, anyhow::Error>(i(n)? as u32);
        match name {
            "glBindBuffer" => match u(0)? {
                glow::ARRAY_BUFFER => self.array = u(1)?,
                glow::ELEMENT_ARRAY_BUFFER => self.elements = u(1)?,
                _ => {}
            },
            "glBufferData" => {
                let id = if u(0)? == glow::ARRAY_BUFFER {
                    self.array
                } else if u(0)? == glow::ELEMENT_ARRAY_BUFFER {
                    self.elements
                } else {
                    return Ok(());
                };
                let len = usize::try_from(i(1)?)?;
                if len > 16 * 1024 * 1024 {
                    bail!("UI buffer limit");
                }
                let data = m.read(u(2)?, len)?;
                self.buffers.insert(id, data);
                if self.buffers.values().map(Vec::len).sum::<usize>() > 32 * 1024 * 1024 {
                    bail!("UI buffers exceed limit");
                }
            }
            "glDeleteBuffers" | "glDeleteTextures" => {
                for n in 0..usize::try_from(i(0)?)? {
                    let id = m.u32(
                        u(1)?
                            .checked_add(n as u32 * 4)
                            .context("UI deletion overflow")?,
                    )?;
                    if name == "glDeleteBuffers" {
                        self.buffers.remove(&id);
                    } else {
                        self.textures.remove(&id);
                    }
                }
            }
            "glActiveTexture" => {
                self.active_texture = u(0)?
                    .checked_sub(glow::TEXTURE0)
                    .context("invalid UI texture unit")?;
                if self.active_texture > 31 {
                    bail!("UI texture unit limit");
                }
                self.texture = self
                    .bindings
                    .get(&self.active_texture)
                    .copied()
                    .unwrap_or(0);
            }
            "glBindTexture" if u(0)? == glow::TEXTURE_2D => {
                self.texture = u(1)?;
                self.bindings.insert(self.active_texture, self.texture);
            }
            "glPixelStorei" if u(0)? == glow::UNPACK_ROW_LENGTH => {
                self.unpack_row_length = usize::try_from(i(1)?)?
            }
            "glPixelStorei" if u(0)? == glow::UNPACK_ALIGNMENT => {
                self.unpack_alignment = usize::try_from(i(1)?)?
            }
            "glTexImage2D" if u(0)? == glow::TEXTURE_2D && i(1)? == 0 => {
                if u(7)? != glow::UNSIGNED_BYTE {
                    bail!("UI texture pixel type");
                }
                let (w, h) = (u(3)?, u(4)?);
                if w == 0 || h == 0 {
                    self.textures.remove(&self.texture);
                    return Ok(());
                }
                let mut rgba = read_pixels(
                    m,
                    (w, h),
                    u(6)?,
                    u(8)?,
                    (self.unpack_row_length, self.unpack_alignment),
                )?;
                apply_internal_format(&mut rgba, u(2)?)?;
                self.internal_formats.insert(self.texture, u(2)?);
                self.version += 1;
                self.textures.insert(
                    self.texture,
                    Arc::new(Texture {
                        version: self.version,
                        width: w,
                        height: h,
                        rgba,
                    }),
                );
                if self.textures.values().map(|t| t.rgba.len()).sum::<usize>() > 64 * 1024 * 1024 {
                    bail!("UI textures exceed limit");
                }
            }
            "glTexSubImage2D" if self.textures.contains_key(&self.texture) => {
                if i(1)? != 0 || u(7)? != glow::UNSIGNED_BYTE {
                    bail!("UI texture update type");
                }
                let (x, y, w, h) = (u(2)?, u(3)?, u(4)?, u(5)?);
                let mut data = read_pixels(
                    m,
                    (w, h),
                    u(6)?,
                    u(8)?,
                    (self.unpack_row_length, self.unpack_alignment),
                )?;
                apply_internal_format(&mut data, self.internal_formats[&self.texture])?;
                let t = self.textures.get_mut(&self.texture).unwrap();
                if x.checked_add(w).is_none_or(|r| r > t.width)
                    || y.checked_add(h).is_none_or(|b| b > t.height)
                {
                    bail!("UI texture update bounds");
                }
                self.version += 1;
                let t = Arc::make_mut(t);
                t.version = self.version;
                for row in 0..h as usize {
                    let start = ((row + y as usize) * t.width as usize + x as usize) * 4;
                    t.rgba[start..start + w as usize * 4]
                        .copy_from_slice(&data[row * w as usize * 4..(row + 1) * w as usize * 4]);
                }
            }
            "glGetAttribLocation" if result.is_some_and(|v| v >= 0) => {
                self.semantics
                    .entry(u(0)?)
                    .or_default()
                    .insert(m.string(u(1)?, 256)?, result.unwrap() as u32);
            }
            "glGetUniformLocation" => {
                let uniform = m.string(u(1)?, 256)?;
                let handle = result.context("UI uniform handle")? as u32;
                if uniform == "proj" {
                    self.uniforms.insert(handle, u(0)?);
                }
                if uniform == "tex" {
                    self.sampler_uniforms.insert(handle, u(0)?);
                }
            }
            "glUniform1i" if self.sampler_uniforms.contains_key(&u(0)?) => {
                self.samplers.insert(self.sampler_uniforms[&u(0)?], u(1)?);
            }
            "glUseProgram" => self.program = u(0)?,
            "glUniformMatrix4fv" if self.uniforms.contains_key(&u(0)?) => {
                if i(1)? != 1 || i(2)? != 0 {
                    bail!("UI projection format");
                }
                let bytes = m.read(u(3)?, 64)?;
                let mut values = [0.; 16];
                for (n, b) in bytes.as_chunks::<4>().0.iter().enumerate() {
                    values[n] = f32::from_le_bytes(*b);
                }
                if values.iter().any(|v| !v.is_finite()) {
                    bail!("invalid UI projection");
                }
                self.projections.insert(self.uniforms[&u(0)?], values);
            }
            "glVertexAttribPointer" => {
                self.attributes.insert(
                    u(0)?,
                    Attribute {
                        buffer: self.array,
                        size: i(1)?,
                        kind: u(2)?,
                        normalized: i(3)? != 0,
                        stride: usize::try_from(i(4)?)?,
                        offset: usize::try_from(i(5)?)?,
                    },
                );
            }
            "glViewport" => self.viewport = [i(0)?, i(1)?, i(2)?, i(3)?],
            "glScissor" => self.clip = [i(0)?, i(1)?, i(2)?, i(3)?],
            "glEnable" if u(0)? == glow::SCISSOR_TEST => self.scissor = Some(self.clip),
            "glDisable" if u(0)? == glow::SCISSOR_TEST => self.scissor = None,
            "glClear" if u(0)? & glow::COLOR_BUFFER_BIT != 0 => {
                self.frame = Frame::default();
                self.bytes = 0;
            }
            "glDrawElements" => self.draw(
                u(0)?,
                usize::try_from(i(1)?)?,
                u(2)?,
                usize::try_from(i(3)?)?,
            )?,
            _ => {}
        }
        if name == "glScissor" && self.scissor.is_some() {
            self.scissor = Some(self.clip);
        }
        Ok(())
    }
    fn draw(&mut self, mode: u32, count: usize, kind: u32, offset: usize) -> Result<()> {
        let Some(semantics) = self.semantics.get(&self.program) else {
            return Ok(());
        };
        if !["pos", "uv", "col"]
            .iter()
            .all(|n| semantics.contains_key(*n))
        {
            return Ok(());
        };
        if count == 0 {
            return Ok(());
        }
        if mode != glow::TRIANGLES || count > 300_000 || self.frame.batches.len() >= 512 {
            bail!("UI draw limit");
        }
        let size = match kind {
            glow::UNSIGNED_SHORT => 2,
            glow::UNSIGNED_INT => 4,
            _ => bail!("UI index format"),
        };
        let indices = self
            .buffers
            .get(&self.elements)
            .context("UI indices missing")?;
        let end = offset
            .checked_add(count.checked_mul(size).context("UI count overflow")?)
            .context("UI offset overflow")?;
        let indices = indices
            .get(offset..end)
            .context("UI indices out of bounds")?;
        let attrs = [
            *self
                .attributes
                .get(&semantics["pos"])
                .context("UI position attribute missing")?,
            *self
                .attributes
                .get(&semantics["uv"])
                .context("UI UV attribute missing")?,
            *self
                .attributes
                .get(&semantics["col"])
                .context("UI color attribute missing")?,
        ];
        for (n, a) in attrs.iter().enumerate() {
            if a.stride == 0
                || a.stride > 256
                || a.size != if n == 2 { 4 } else { 2 }
                || a.kind
                    != if n == 2 {
                        glow::UNSIGNED_BYTE
                    } else {
                        glow::FLOAT
                    }
                || a.normalized != (n == 2)
            {
                bail!("UI vertex format");
            }
        }
        let matrix = self
            .projections
            .get(&self.program)
            .context("UI projection missing")?;
        let unit = self.samplers.get(&self.program).copied().unwrap_or(0);
        let texture_id = self.bindings.get(&unit).copied().unwrap_or(0);
        let texture = if texture_id == 0 {
            // GLES/WebGL incomplete default texture samples opaque black.
            // Match that result rather than treating a zero binding as missing data.
            Arc::new(Texture {
                version: 0,
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            })
        } else {
            self.textures
                .get(&texture_id)
                .context("UI nondefault texture missing")?
                .clone()
        };
        let mut vertices = Vec::with_capacity(count * 20);
        for index in indices.chunks_exact(size) {
            let index = if size == 2 {
                u16::from_le_bytes(index.try_into()?) as usize
            } else {
                u32::from_le_bytes(index.try_into()?) as usize
            };
            let mut values = [[0u8; 8]; 3];
            for (n, a) in attrs.iter().enumerate() {
                let start = index
                    .checked_mul(a.stride)
                    .and_then(|v| v.checked_add(a.offset))
                    .context("UI vertex overflow")?;
                let len = if n == 2 { 4 } else { 8 };
                let data = self
                    .buffers
                    .get(&a.buffer)
                    .and_then(|b| b.get(start..start + len))
                    .context("UI vertex out of bounds")?;
                values[n][..len].copy_from_slice(data);
            }
            let x = f32::from_le_bytes(values[0][..4].try_into()?);
            let y = f32::from_le_bytes(values[0][4..].try_into()?);
            let px = matrix[0] * x + matrix[4] * y + matrix[12];
            let py = matrix[1] * x + matrix[5] * y + matrix[13];
            if !px.is_finite() || !py.is_finite() {
                bail!("invalid UI position");
            }
            vertices.extend_from_slice(&px.to_le_bytes());
            vertices.extend_from_slice(&py.to_le_bytes());
            vertices.extend_from_slice(&values[1]);
            vertices.extend_from_slice(&values[2][..4]);
        }
        self.bytes += vertices.len();
        if self.bytes > 8 * 1024 * 1024 {
            bail!("UI frame bytes limit");
        }
        let [vx, vy, vw, vh] = self.viewport;
        if vx != 0 || vy != 0 || vw <= 0 || vh <= 0 || vw > 16384 || vh > 16384 {
            bail!("UI viewport unsupported");
        }
        self.frame.size = (vw as u32, vh as u32);
        let [x, y, w, h] = self.scissor.unwrap_or([0, 0, vw, vh]);
        self.frame.batches.push(Batch {
            vertices,
            texture,
            clip: [x, vh - y - h, x + w, vh - y],
        });
        Ok(())
    }
}

fn read_pixels(
    m: &GuestMemory,
    size: (u32, u32),
    format: u32,
    pointer: u32,
    storage: (usize, usize),
) -> Result<Vec<u8>> {
    let (w, h) = (size.0 as usize, size.1 as usize);
    if w > 4096 || h > 4096 {
        bail!("UI texture size");
    }
    let channels = match format {
        glow::RGBA => 4,
        glow::RGB => 3,
        glow::RG | glow::LUMINANCE_ALPHA => 2,
        glow::RED | glow::ALPHA | glow::LUMINANCE => 1,
        _ => bail!("UI texture pixel format"),
    };
    let row_width = if storage.0 == 0 { w } else { storage.0 };
    let alignment = if storage.1 == 0 { 4 } else { storage.1 };
    if row_width < w || row_width > 4096 || ![1, 2, 4, 8].contains(&alignment) {
        bail!("UI texture pixel storage");
    }
    let stride = (row_width * channels).div_ceil(alignment) * alignment;
    let count = if h == 0 {
        0
    } else {
        stride * (h - 1) + w * channels
    };
    if count > 16 * 1024 * 1024 {
        bail!("UI upload byte limit");
    }
    let data = if pointer == 0 {
        vec![0; count]
    } else {
        m.read(pointer, count)?
    };
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        for x in 0..w {
            let pixel = &data[row * stride + x * channels..row * stride + (x + 1) * channels];
            let color = match format {
                glow::RGBA => [pixel[0], pixel[1], pixel[2], pixel[3]],
                glow::RGB => [pixel[0], pixel[1], pixel[2], 255],
                glow::RED => [pixel[0], 0, 0, 255],
                glow::RG => [pixel[0], pixel[1], 0, 255],
                glow::ALPHA => [0, 0, 0, pixel[0]],
                glow::LUMINANCE => [pixel[0], pixel[0], pixel[0], 255],
                glow::LUMINANCE_ALPHA => [pixel[0], pixel[0], pixel[0], pixel[1]],
                _ => unreachable!(),
            };
            rgba.extend_from_slice(&color);
        }
    }
    Ok(rgba)
}
fn apply_internal_format(pixels: &mut [u8], format: u32) -> Result<()> {
    for p in pixels.as_chunks_mut::<4>().0 {
        match format {
            glow::RGBA | glow::RGBA8 => {}
            glow::RGB | glow::RGB8 => p[3] = 255,
            glow::ALPHA => {
                p[0] = 0;
                p[1] = 0;
                p[2] = 0;
            }
            glow::LUMINANCE => {
                p[1] = p[0];
                p[2] = p[0];
                p[3] = 255;
            }
            glow::LUMINANCE_ALPHA => {
                p[1] = p[0];
                p[2] = p[0];
            }
            glow::RED | glow::R8 => {
                p[1] = 0;
                p[2] = 0;
                p[3] = 255;
            }
            glow::RG | glow::RG8 => {
                p[2] = 0;
                p[3] = 255;
            }
            _ => bail!("UI texture internal format"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn triangle() -> Capture {
        let mut capture = Capture {
            program: 1,
            elements: 2,
            texture: 3,
            viewport: [0, 0, 100, 100],
            scissor: Some([20, 30, 40, 50]),
            ..Default::default()
        };
        let mut vertices = Vec::new();
        for (x, y) in [(0.0f32, 0.0f32), (1., 0.), (0., 1.)] {
            vertices.extend_from_slice(&x.to_le_bytes());
            vertices.extend_from_slice(&y.to_le_bytes());
            vertices.extend_from_slice(&[0; 8]);
            vertices.extend_from_slice(&[255, 255, 255, 255]);
        }
        capture.buffers.insert(1, vertices);
        capture.buffers.insert(2, vec![0, 0, 1, 0, 2, 0]);
        capture.semantics.insert(
            1,
            BTreeMap::from([("pos".into(), 0), ("uv".into(), 1), ("col".into(), 2)]),
        );
        for (location, offset) in [(0, 0), (1, 8), (2, 16)] {
            capture.attributes.insert(
                location,
                Attribute {
                    buffer: 1,
                    size: if location == 2 { 4 } else { 2 },
                    kind: if location == 2 {
                        glow::UNSIGNED_BYTE
                    } else {
                        glow::FLOAT
                    },
                    normalized: location == 2,
                    stride: 20,
                    offset,
                },
            );
        }
        capture.projections.insert(
            1,
            [
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
        );
        capture.bindings.insert(0, 3);
        capture.textures.insert(
            3,
            Arc::new(Texture {
                version: 1,
                width: 1,
                height: 1,
                rgba: vec![255; 4],
            }),
        );
        capture
    }
    #[test]
    fn gui_transform_scissor_and_empty_frame_publication_preserve_layering() {
        let mut capture = triangle();
        capture
            .draw(glow::TRIANGLES, 3, glow::UNSIGNED_SHORT, 0)
            .unwrap();
        assert_eq!(capture.frame.batches[0].clip, [20, 20, 60, 70]);
        assert_eq!(capture.frame.batches[0].vertices.len(), 60);
        assert_eq!(
            &capture.frame.batches[0].vertices[20..24],
            &1.0f32.to_le_bytes()
        );
        let mut shared = Shared::default();
        capture.publish(&mut shared);
        assert_eq!(shared.frame.batches.len(), 1);
        capture.publish(&mut shared);
        assert!(shared.frame.batches.is_empty());
    }
    #[test]
    fn legacy_alpha_internal_format_preserves_alpha_and_zeroes_color() {
        let mut pixels = [10, 20, 30, 40, 255, 255, 255, 127];
        apply_internal_format(&mut pixels, glow::ALPHA).unwrap();
        assert_eq!(pixels, [0, 0, 0, 40, 0, 0, 0, 127]);
    }
    #[test]
    fn gui_sampler_selects_its_bound_texture_unit() {
        let mut capture = triangle();
        capture.bindings.insert(0, 999);
        capture.bindings.insert(2, 3);
        capture.samplers.insert(1, 2);
        capture
            .draw(glow::TRIANGLES, 3, glow::UNSIGNED_SHORT, 0)
            .unwrap();
        assert_eq!(capture.frame.batches[0].texture.version, 1);
        capture.bindings.remove(&2);
        capture
            .draw(glow::TRIANGLES, 3, glow::UNSIGNED_SHORT, 0)
            .unwrap();
        assert_eq!(capture.frame.batches[1].texture.rgba, [0, 0, 0, 255]);
        capture.bindings.insert(2, 999);
        assert!(capture
            .draw(glow::TRIANGLES, 3, glow::UNSIGNED_SHORT, 0)
            .is_err());
    }
    #[test]
    fn bad_indices_and_missing_layout_fail_without_panics_or_stale_overlay() {
        let mut capture = triangle();
        assert!(capture
            .draw(glow::TRIANGLES, 4, glow::UNSIGNED_SHORT, 0)
            .is_err());
        capture.attributes.remove(&0);
        assert!(capture
            .draw(glow::TRIANGLES, 3, glow::UNSIGNED_SHORT, 0)
            .is_err());
        let mut shared = Shared {
            frame: Arc::new(Frame {
                size: (100, 100),
                batches: vec![Batch {
                    vertices: vec![0; 60],
                    texture: capture.textures[&3].clone(),
                    clip: [0; 4],
                }],
            }),
            ..Default::default()
        };
        capture.failed = true;
        capture.publish(&mut shared);
        assert!(shared.frame.batches.is_empty());
        assert!(shared.report.failure.is_some());
    }
}
