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
}
impl Capture {
    pub fn publish(&mut self, shared: &mut Shared) {
        if self.failed {
            shared.frame = Arc::default();
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
        if !self.failed && self.record(name, m, args, result).is_err() {
            self.failed = true;
            self.frame = Frame::default();
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
            "glActiveTexture" => self.active_texture = u(0)?.saturating_sub(glow::TEXTURE0),
            "glBindTexture" if self.active_texture == 0 && u(0)? == glow::TEXTURE_2D => {
                self.texture = u(1)?
            }
            "glTexImage2D"
                if self.active_texture == 0 && u(0)? == glow::TEXTURE_2D && i(1)? == 0 =>
            {
                if u(6)? != glow::RGBA || u(7)? != glow::UNSIGNED_BYTE {
                    return Ok(());
                }
                let (w, h) = (u(3)?, u(4)?);
                if w == 0 || h == 0 || w > 4096 || h > 4096 {
                    bail!("UI texture size");
                }
                let len = w as usize * h as usize * 4;
                let rgba = if u(8)? == 0 {
                    vec![0; len]
                } else {
                    m.read(u(8)?, len)?
                };
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
            "glTexSubImage2D"
                if self.active_texture == 0 && self.textures.contains_key(&self.texture) =>
            {
                if i(1)? != 0 || u(6)? != glow::RGBA || u(7)? != glow::UNSIGNED_BYTE {
                    bail!("UI texture update format");
                }
                let (x, y, w, h) = (u(2)?, u(3)?, u(4)?, u(5)?);
                let t = self.textures.get_mut(&self.texture).unwrap();
                if x.checked_add(w).is_none_or(|r| r > t.width)
                    || y.checked_add(h).is_none_or(|b| b > t.height)
                {
                    bail!("UI texture update bounds");
                }
                let data = m.read(u(8)?, w as usize * h as usize * 4)?;
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
            "glGetUniformLocation" if m.string(u(1)?, 256)? == "proj" => {
                self.uniforms
                    .insert(result.context("UI uniform handle")? as u32, u(0)?);
            }
            "glUseProgram" => self.program = u(0)?,
            "glUniformMatrix4fv" if self.uniforms.contains_key(&u(0)?) => {
                if i(1)? != 1 || i(2)? != 0 {
                    bail!("UI projection format");
                }
                let bytes = m.read(u(3)?, 64)?;
                let mut values = [0.; 16];
                for (n, b) in bytes.chunks_exact(4).enumerate() {
                    values[n] = f32::from_le_bytes(b.try_into()?);
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
        let texture = self
            .textures
            .get(&self.texture)
            .context("UI texture missing")?
            .clone();
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

#[cfg(test)]
mod tests {
    use super::*;
    fn triangle() -> Capture {
        let mut capture = Capture::default();
        capture.program = 1;
        capture.elements = 2;
        capture.texture = 3;
        capture.viewport = [0, 0, 100, 100];
        capture.scissor = Some([20, 30, 40, 50]);
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
