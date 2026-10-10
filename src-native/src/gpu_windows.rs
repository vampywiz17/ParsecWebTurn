//! Public DXGI hardware enumeration; no vendor SDK or global GPU override.
use crate::connection_settings::GpuPreference;
use windows::Wdk::Graphics::Direct3D::*;
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Dxgi::*;

pub struct Adapter {
    pub preference: GpuPreference,
    pub handle: IDXGIAdapter1,
}
pub fn enumerate() -> windows::core::Result<Vec<Adapter>> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut adapters = Vec::new();
        for index in 0..64 {
            let handle = match factory.EnumAdapters1(index) {
                Ok(adapter) => adapter,
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(error) => return Err(error),
            };
            let desc = handle.GetDesc1()?;
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            if is_display_only_indirect(adapter_type(desc.AdapterLuid)) {
                continue;
            }
            adapters.push(Adapter {
                preference: GpuPreference {
                    name: String::from_utf16_lossy(&desc.Description)
                        .trim_end_matches('\0')
                        .into(),
                    vendor_id: desc.VendorId,
                    device_id: desc.DeviceId,
                    subsystem_id: desc.SubSysId,
                    revision: desc.Revision,
                    luid: ((desc.AdapterLuid.HighPart as u32 as u64) << 32)
                        | u64::from(desc.AdapterLuid.LowPart),
                },
                handle,
            });
        }
        Ok(adapters)
    }
}

// Indirect display drivers can appear in DXGI under the backing GPU's name,
// with a separate LUID, even exposing its video profiles. Query the documented
// kernel adapter type instead of merging names or probing decoder support.
fn adapter_type(luid: LUID) -> Option<u32> {
    unsafe {
        let mut opened = D3DKMT_OPENADAPTERFROMLUID {
            AdapterLuid: luid,
            ..Default::default()
        };
        if D3DKMTOpenAdapterFromLuid(&mut opened).is_err() {
            return None;
        }
        let mut kind = D3DKMT_ADAPTERTYPE::default();
        let mut query = D3DKMT_QUERYADAPTERINFO {
            hAdapter: opened.hAdapter,
            Type: KMTQAITYPE_ADAPTERTYPE,
            pPrivateDriverData: std::ptr::from_mut(&mut kind).cast(),
            PrivateDriverDataSize: std::mem::size_of_val(&kind) as u32,
        };
        let result = D3DKMTQueryAdapterInfo(&mut query);
        let _ = D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER {
            hAdapter: opened.hAdapter,
        });
        result.is_ok().then_some(kind.Anonymous.Value)
    }
}

fn is_display_only_indirect(kind: Option<u32>) -> bool {
    const RENDER_SUPPORTED: u32 = 1;
    const INDIRECT_DISPLAY_DEVICE: u32 = 1 << 6;
    kind.is_some_and(|flags| flags & INDIRECT_DISPLAY_DEVICE != 0 && flags & RENDER_SUPPORTED == 0)
}

// LUIDs can change after reboot. Only accept a unique hardware match in that
// case; two identical GPUs must never silently select an arbitrary adapter.
pub fn match_index(preferred: &GpuPreference, adapters: &[GpuPreference]) -> Option<usize> {
    if let Some(index) = adapters
        .iter()
        .position(|p| p.luid == preferred.luid && p.same_hardware(preferred))
    {
        return Some(index);
    }
    let mut matches = adapters
        .iter()
        .enumerate()
        .filter(|(_, p)| p.same_hardware(preferred));
    let (index, _) = matches.next()?;
    matches.next().is_none().then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excludes_indirect_display_alias_but_keeps_render_devices_and_unknowns() {
        assert!(is_display_only_indirect(Some(0x0342)));
        assert!(!is_display_only_indirect(Some(0x232b))); // integrated GPU
        assert!(!is_display_only_indirect(Some(0x2313))); // discrete GPU
        assert!(!is_display_only_indirect(Some(0x0343))); // render-capable indirect
        assert!(!is_display_only_indirect(None)); // query failure is not evidence
    }
    #[test]
    fn adapter_identity_survives_reorder_and_unique_reboot_but_not_ambiguity() {
        let gpu = GpuPreference {
            name: "GPU".into(),
            vendor_id: 1,
            device_id: 2,
            subsystem_id: 3,
            revision: 4,
            luid: 5,
        };
        let other = GpuPreference {
            device_id: 9,
            ..gpu.clone()
        };
        assert_eq!(match_index(&gpu, &[other, gpu.clone()]), Some(1));
        let rebooted = GpuPreference {
            luid: 6,
            ..gpu.clone()
        };
        assert_eq!(match_index(&gpu, std::slice::from_ref(&rebooted)), Some(0));
        assert_eq!(match_index(&gpu, &[rebooted.clone(), rebooted]), None);
        assert_eq!(match_index(&gpu, &[]), None);
    }
}
