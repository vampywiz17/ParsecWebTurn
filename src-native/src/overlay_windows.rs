//! D3D11 alpha-blended Matoya GUI over the existing converted video backbuffer.
//! Uploads contain GUI vertices/atlases only, never decoded video pixels.
use crate::overlay::{Frame, Texture};
use std::{collections::BTreeMap, sync::Arc};
use windows::{
    core::{s, PCSTR},
    Win32::{
        Foundation::RECT,
        Graphics::{
            Direct3D::{Fxc::*, *},
            Direct3D11::*,
            Dxgi::Common::*,
        },
    },
};
pub struct Renderer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    layout: ID3D11InputLayout,
    blend: ID3D11BlendState,
    depth: ID3D11DepthStencilState,
    raster: ID3D11RasterizerState,
    sampler: ID3D11SamplerState,
    buffer: Option<ID3D11Buffer>,
    capacity: usize,
    uploaded: Option<Arc<Frame>>,
    textures: BTreeMap<u64, ID3D11ShaderResourceView>,
}
const SHADER: &str = "struct V { float2 p:POSITION; float2 uv:TEXCOORD; float4 c:COLOR; }; struct P { float4 p:SV_POSITION; float2 uv:TEXCOORD; float4 c:COLOR; }; P vs(V v) { P p; p.p=float4(v.p,0,1); p.uv=v.uv; p.c=v.c; return p; } Texture2D tex:register(t0); SamplerState samp:register(s0); float4 ps(P p):SV_TARGET { return p.c*tex.Sample(samp,p.uv); }";
unsafe fn compile(entry: PCSTR, target: PCSTR) -> windows::core::Result<ID3DBlob> {
    let mut code = None;
    D3DCompile(
        SHADER.as_ptr().cast(),
        SHADER.len(),
        PCSTR::null(),
        None,
        None,
        entry,
        target,
        D3DCOMPILE_ENABLE_STRICTNESS,
        0,
        &mut code,
        None,
    )?;
    code.ok_or_else(windows::core::Error::from_thread)
}
unsafe fn blob(blob: &ID3DBlob) -> &[u8] {
    std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize())
}
impl Renderer {
    pub unsafe fn create(device: &ID3D11Device) -> windows::core::Result<Self> {
        let context = device.GetImmediateContext()?;
        let vertex = compile(s!("vs"), s!("vs_4_0"))?;
        let pixel = compile(s!("ps"), s!("ps_4_0"))?;
        let mut vs = None;
        device.CreateVertexShader(blob(&vertex), None, Some(&mut vs))?;
        let mut ps = None;
        device.CreatePixelShader(blob(&pixel), None, Some(&mut ps))?;
        let attrs = [
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("POSITION"),
                Format: DXGI_FORMAT_R32G32_FLOAT,
                AlignedByteOffset: 0,
                ..Default::default()
            },
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("TEXCOORD"),
                Format: DXGI_FORMAT_R32G32_FLOAT,
                AlignedByteOffset: 8,
                ..Default::default()
            },
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("COLOR"),
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                AlignedByteOffset: 16,
                ..Default::default()
            },
        ];
        let mut layout = None;
        device.CreateInputLayout(&attrs, blob(&vertex), Some(&mut layout))?;
        let mut desc = D3D11_BLEND_DESC::default();
        desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: true.into(),
            SrcBlend: D3D11_BLEND_SRC_ALPHA,
            DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D11_BLEND_OP_ADD,
            SrcBlendAlpha: D3D11_BLEND_ONE,
            DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOpAlpha: D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
        let mut blend = None;
        device.CreateBlendState(&desc, Some(&mut blend))?;
        let desc = D3D11_RASTERIZER_DESC {
            FillMode: D3D11_FILL_SOLID,
            CullMode: D3D11_CULL_NONE,
            ScissorEnable: true.into(),
            DepthClipEnable: true.into(),
            ..Default::default()
        };
        let mut depth = None;
        device.CreateDepthStencilState(
            &D3D11_DEPTH_STENCIL_DESC {
                DepthEnable: false.into(),
                DepthWriteMask: D3D11_DEPTH_WRITE_MASK_ZERO,
                DepthFunc: D3D11_COMPARISON_ALWAYS,
                FrontFace: D3D11_DEPTH_STENCILOP_DESC {
                    StencilFailOp: D3D11_STENCIL_OP_KEEP,
                    StencilDepthFailOp: D3D11_STENCIL_OP_KEEP,
                    StencilPassOp: D3D11_STENCIL_OP_KEEP,
                    StencilFunc: D3D11_COMPARISON_ALWAYS,
                },
                BackFace: D3D11_DEPTH_STENCILOP_DESC {
                    StencilFailOp: D3D11_STENCIL_OP_KEEP,
                    StencilDepthFailOp: D3D11_STENCIL_OP_KEEP,
                    StencilPassOp: D3D11_STENCIL_OP_KEEP,
                    StencilFunc: D3D11_COMPARISON_ALWAYS,
                },
                ..Default::default()
            },
            Some(&mut depth),
        )?;
        let mut raster = None;
        device.CreateRasterizerState(&desc, Some(&mut raster))?;
        let desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            ComparisonFunc: D3D11_COMPARISON_ALWAYS,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut sampler = None;
        device.CreateSamplerState(&desc, Some(&mut sampler))?;
        Ok(Self {
            device: device.clone(),
            context,
            vs: vs.unwrap(),
            ps: ps.unwrap(),
            layout: layout.unwrap(),
            blend: blend.unwrap(),
            depth: depth.unwrap(),
            raster: raster.unwrap(),
            sampler: sampler.unwrap(),
            buffer: None,
            capacity: 0,
            uploaded: None,
            textures: BTreeMap::new(),
        })
    }
    unsafe fn texture(
        &mut self,
        texture: &Texture,
    ) -> windows::core::Result<ID3D11ShaderResourceView> {
        if let Some(view) = self.textures.get(&texture.version) {
            return Ok(view.clone());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: texture.width,
            Height: texture.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: texture.rgba.as_ptr().cast(),
            SysMemPitch: texture.width * 4,
            ..Default::default()
        };
        let mut resource = None;
        self.device
            .CreateTexture2D(&desc, Some(&data), Some(&mut resource))?;
        let mut view = None;
        self.device
            .CreateShaderResourceView(resource.as_ref().unwrap(), None, Some(&mut view))?;
        let view = view.unwrap();
        self.textures.insert(texture.version, view.clone());
        Ok(view)
    }
    pub unsafe fn draw(
        &mut self,
        frame: Arc<Frame>,
        back: &ID3D11Texture2D,
        size: (u32, u32),
    ) -> windows::core::Result<u64> {
        if frame.size != size || frame.batches.is_empty() {
            return Ok(0);
        }
        let total = frame
            .batches
            .iter()
            .map(|b| b.vertices.len())
            .sum::<usize>();
        if !self
            .uploaded
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, &frame))
        {
            if total > self.capacity {
                self.capacity = total.next_power_of_two();
                let desc = D3D11_BUFFER_DESC {
                    ByteWidth: self.capacity as u32,
                    Usage: D3D11_USAGE_DYNAMIC,
                    BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
                    CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                    ..Default::default()
                };
                self.buffer = None;
                self.device
                    .CreateBuffer(&desc, None, Some(&mut self.buffer))?;
            }
            let buffer = self.buffer.as_ref().unwrap();
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))?;
            let destination = std::slice::from_raw_parts_mut(mapped.pData.cast::<u8>(), total);
            let mut offset = 0;
            for batch in &frame.batches {
                destination[offset..offset + batch.vertices.len()].copy_from_slice(&batch.vertices);
                offset += batch.vertices.len();
            }
            self.context.Unmap(buffer, 0);
            self.uploaded = Some(frame.clone());
        }
        let mut target = None;
        self.device
            .CreateRenderTargetView(back, None, Some(&mut target))?;
        let _bindings = Bindings(self.context.clone());
        self.context.OMSetRenderTargets(Some(&[target]), None);
        self.context.OMSetBlendState(&self.blend, None, u32::MAX);
        self.context.OMSetDepthStencilState(&self.depth, 0);
        self.context.RSSetState(&self.raster);
        self.context.RSSetViewports(Some(&[D3D11_VIEWPORT {
            Width: size.0 as f32,
            Height: size.1 as f32,
            MinDepth: 0.,
            MaxDepth: 1.,
            ..Default::default()
        }]));
        self.context.IASetInputLayout(&self.layout);
        self.context
            .IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        self.context
            .IASetVertexBuffers(0, 1, Some(&self.buffer), Some(&20), Some(&0));
        self.context.VSSetShader(&self.vs, None);
        self.context.PSSetShader(&self.ps, None);
        self.context
            .PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
        let mut offset = 0;
        for batch in &frame.batches {
            let texture = self.texture(&batch.texture)?;
            self.context.PSSetShaderResources(0, Some(&[Some(texture)]));
            let [left, top, right, bottom] = batch.clip;
            self.context.RSSetScissorRects(Some(&[RECT {
                left,
                top,
                right,
                bottom,
            }]));
            self.context
                .Draw((batch.vertices.len() / 20) as u32, offset);
            offset += (batch.vertices.len() / 20) as u32;
        }
        self.context.PSSetShaderResources(0, Some(&[None]));
        self.context.OMSetRenderTargets(None, None);
        // Release atlas versions no longer referenced by the latest frame.
        self.textures
            .retain(|version, _| frame.batches.iter().any(|b| b.texture.version == *version));
        Ok(frame.batches.len() as u64)
    }
}

struct Bindings(ID3D11DeviceContext);
impl Drop for Bindings {
    fn drop(&mut self) {
        unsafe {
            self.0.PSSetShaderResources(0, Some(&[None]));
            self.0.OMSetRenderTargets(None, None);
        }
    }
}
