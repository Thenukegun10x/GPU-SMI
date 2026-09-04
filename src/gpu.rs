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

/// PCI device ID -> gfx IP string, verified against the kernel amdgpu
/// device list (GC IP version per die) and ROCm GPU specs. Unknown IDs
/// return "unknown" — honest beats wrong, ROCm keys features off this.
pub fn gfx_for_pci_dev(dev: u32) -> String {
    match dev {
        0x7550 | 0x7551 => "gfx1201",          // Navi48: RX 9070/XT/GRE, AI PRO R9700
        0x7590 => "gfx1200",                   // Navi44: RX 9060/XT
        0x7448..=0x744C | 0x745E => "gfx1100", // Navi31: RX 7900/Pro W78xx/W79xx
        0x7460..=0x747F => "gfx1101",          // Navi32: RX 7800/7700/Pro W7700/V710
        0x7480..=0x749F => "gfx1102",          // Navi33: RX 7600/7500/Pro W75xx
        0x73BF | 0x73A5 => "gfx1030",          // Navi21: RX 6800/6900/6950XT
        0x73DF => "gfx1031",                   // Navi22: RX 6700
        0x73FF => "gfx1032",                   // Navi23: RX 6600
        0x164E => "gfx1036",                   // Raphael iGPU
        0x15BF => "gfx1103",                   // Phoenix/Hawk Point iGPU
        _ => "unknown",
    }
    .to_string()
}

/// C = Compute, G = Graphics. Same name heuristic on every platform so
/// --serve output stays comparable between Windows and Linux.
pub fn classify_proc_type(name: &str) -> String {
    let n = name.to_lowercase();
    if n.contains("python") || n.contains("ollama") || n.contains("llama")
        || n.contains("torch") || n.contains("vllm") || n.contains("triton")
        || n.contains("compute")
    {
        "C".to_string()
    } else {
        "G".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gfx_table_spot_checks() {
        assert_eq!(gfx_for_pci_dev(0x7550), "gfx1201"); // 9070 XT, was correct
        assert_eq!(gfx_for_pci_dev(0x744C), "gfx1100"); // 7900 XTX was mislabeled gfx1201
        assert_eq!(gfx_for_pci_dev(0x7448), "gfx1100"); // Pro W7900, was mislabeled gfx1201
        assert_eq!(gfx_for_pci_dev(0x7460), "gfx1101"); // Pro V710, was mislabeled gfx1201
        assert_eq!(gfx_for_pci_dev(0x747E), "gfx1101"); // 7800 XT, was mislabeled gfx1200
        assert_eq!(gfx_for_pci_dev(0x7480), "gfx1102"); // 7600
        assert_eq!(gfx_for_pci_dev(0x7590), "gfx1200"); // 9060 XT
        assert_eq!(gfx_for_pci_dev(0x73BF), "gfx1030"); // 6900 XT, was mislabeled gfx1100
        assert_eq!(gfx_for_pci_dev(0x164E), "gfx1036"); // Raphael, was mislabeled gfx1151
        assert_eq!(gfx_for_pci_dev(0x15BF), "gfx1103"); // Phoenix, was mislabeled gfx1151
        assert_eq!(gfx_for_pci_dev(0x1234), "unknown");
    }

    #[test]
    fn proc_type_spot_checks() {
        assert_eq!(classify_proc_type("python3.11"), "C");
        assert_eq!(classify_proc_type("ollama"), "C");
        assert_eq!(classify_proc_type("Discord.exe"), "G");
        assert_eq!(classify_proc_type("firefox"), "G");
    }
}
