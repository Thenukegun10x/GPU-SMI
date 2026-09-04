use serde::Serialize;

// Clean portable mirror of amdsmi.h structs (amdsmi_asic_info_t, amdsmi_pcie_info_t, etc)
// Reference: ../amdsmi/include/amd_smi/amdsmi.h (cloned for reference only - no link dependency)

#[derive(Debug, Clone, Serialize)]
pub struct GpuInfo {
    pub index: u32,
    /// Mirrors amdsmi_asic_info_t.market_name
    pub market_name: String,
    pub vendor_id: u32,
    pub vendor_name: String,
    pub device_id: u64,
    pub subsystem_id: u32,
    pub rev_id: u32,
    pub asic_serial: String,
    pub num_cu: u32,
    pub gfx_version: String, // target_graphics_version as "gfx1201"
    pub vram_type: String,
    pub vram_total_mb: u32,
    pub vram_used_mb: u32,
    pub vram_pinned_mb: u32,
    pub vram_vendor: String,
    pub bdf: String, // "0000:03:00.0"
    pub pcie_width: u16,
    pub pcie_speed_gt: u32,
    pub driver_version: String,
    pub vbios_version: String,
    pub temp_edge_c: Option<f32>,
    pub temp_hotspot_c: Option<f32>,
    pub temp_vram_c: Option<f32>,
    pub gfx_clock_mhz: Option<u32>,
    pub mem_clock_mhz: Option<u32>,
    pub gfx_util_percent: Option<u32>,
    pub power_w: Option<f32>,
    pub power_cap_w: Option<f32>,
    pub backend: String, // "amdsmi" | "hip" | "adl" | "wmi" | "sysfs"
    pub processes: Vec<ProcessInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub mem_used_mb: u32,
    pub proc_type: String, // "G" (Graphics) | "C" (Compute)
}

impl GpuInfo {
    pub fn gfx_version_str(raw: u64) -> String {
        if raw == 0xFFFFFFFFFFFFFFFF || raw == 0xFFFFFFFF || raw == 0 {
            return "unknown".to_string();
        }
        // AMD encodes as e.g. 0x1201 for gfx1201 -> decode as hex gfx version
        // amdsmi stores as integer, but we display gfxXXXX
        format!("gfx{:x}", raw)
    }
}
