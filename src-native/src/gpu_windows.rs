//! Public DXGI hardware enumeration; no vendor SDK or global GPU override.
use crate::connection_settings::GpuPreference;
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
