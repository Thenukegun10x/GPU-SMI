pub mod linux;
pub mod windows;
pub mod hip;

use crate::gpu::GpuInfo;

pub trait Backend {
    fn discover(&self) -> anyhow::Result<Vec<GpuInfo>>;
}

pub fn discover_all() -> Vec<GpuInfo> {
    // Try native backend first (amdsmi on Linux, ADL/HIP/WMI on Windows), fallback chain
    #[cfg(target_os = "linux")]
    {
        // amdsmi when present, else sysfs sensors enriched with HIP names + fdinfo processes
        if let Ok(gpus) = linux::LinuxAmdSmiBackend.discover() {
            if !gpus.is_empty() { return gpus; }
        }
        let mut gpus = linux::LinuxSysfsBackend.discover().unwrap_or_default();
        if gpus.is_empty() {
            return hip::HipBackend.discover().unwrap_or_default();
        }
        linux::enrich_with_hip(&mut gpus);
        linux::enrich_processes_linux(&mut gpus);
        gpus
    }
    #[cfg(target_os = "windows")]
    {
        // Merge HIP (VRAM, name, gfx) + WMI (device_id, driver, BDF) for full amdsmi_asic_info_t
        let hip = hip::HipBackend.discover().unwrap_or_default();
        let wmi = windows::WmiBackend.discover().unwrap_or_default();
        let mut merged = if !hip.is_empty() && !wmi.is_empty() {
            let mut merged = Vec::new();
            for (i, mut h) in hip.into_iter().enumerate() {
                if let Some(w) = wmi.get(i) {
                    h.device_id = w.device_id;
                    h.subsystem_id = w.subsystem_id;
                    h.rev_id = w.rev_id;
                    h.bdf = w.bdf.clone();
                    h.driver_version = w.driver_version.clone();
                    if h.gfx_version == "unknown" { h.gfx_version = w.gfx_version.clone(); }
                    if h.vram_total_mb == 0 { h.vram_total_mb = w.vram_total_mb; }
                    h.backend = format!("hip+wmi");
                }
                merged.push(h);
            }
            if wmi.len() > merged.len() {
                merged.extend(wmi.into_iter().skip(merged.len()));
            }
            merged
        } else if !hip.is_empty() { hip } else if !wmi.is_empty() { wmi } else {
            if let Ok(gpus) = windows::WindowsAdlBackend.discover() { if !gpus.is_empty() { return gpus; } }
            Vec::new()
        };
        // Enrich with ADL PMLog temps/clocks/power/util/pcie (best-effort)
        windows::enrich_with_adl(&mut merged);
        // Enrich with accurate VRAM usage and OS pinned VRAM via WDDM/DXGI
        windows::enrich_vram_usage(&mut merged);
        return merged;
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        hip::HipBackend.discover().unwrap_or_default()
    }
}
