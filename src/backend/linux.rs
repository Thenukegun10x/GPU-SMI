use crate::gpu::GpuInfo;
use super::Backend;
use libloading::{Library, Symbol};
use std::ffi::CString;

// Minimal dynamic amdsmi bindings - mirrors amdsmi.h
// Reference headers in ../amdsmi/include/amd_smi/amdsmi.h (MIT)
// We dlopen libamd_smi.so at runtime so binary stays portable if library missing

pub struct LinuxAmdSmiBackend;
pub struct LinuxSysfsBackend;

impl Backend for LinuxAmdSmiBackend {
    fn discover(&self) -> anyhow::Result<Vec<GpuInfo>> {
        unsafe {
            let lib = Library::new("libamd_smi.so").map_err(|e| anyhow::anyhow!(e))?;
            let init: Symbol<unsafe extern "C" fn(u64) -> i32> = lib.get(b"amdsmi_init")?;
            let shut: Symbol<unsafe extern "C" fn() -> i32> = lib.get(b"amdsmi_shut_down")?;
            let get_socket_handles: Symbol<unsafe extern "C" fn(*mut u32, *mut *mut std::ffi::c_void) -> i32> = lib.get(b"amdsmi_get_socket_handles")?;

            // Simplified: init with AMD_GPUS (1<<1)
            let ret = init(1u64<<1);
            if ret != 0 { anyhow::bail!("amdsmi_init failed {}", ret); }

            // For portability, we fallback to sysfs if enum fails
            // Full enum requires processor_handle iteration (amdsmi_get_processor_handles etc)
            // To keep single-file diff small, we prove linkage and then delegate to sysfs which
            // is more reliable without KFD running as root
            let _ = shut();
            anyhow::bail!("amdsmi enum not yet expanded - using sysfs fallback")
        }
    }
}

impl Backend for LinuxSysfsBackend {
    fn discover(&self) -> anyhow::Result<Vec<GpuInfo>> {
        let mut gpus = Vec::new();
        for entry in std::fs::read_dir("/sys/class/drm").unwrap_or_else(|_| std::fs::read_dir("/tmp").unwrap()) {
            let entry = match entry { Ok(e) => e, Err(_) => continue };
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("card") || name.contains("render") { continue; }
            let dev_path = entry.path().join("device");
            if !dev_path.exists() { continue; }
            let vendor = std::fs::read_to_string(dev_path.join("vendor")).unwrap_or_default();
            if !vendor.trim().eq_ignore_ascii_case("0x1002") { continue; }
            let device = std::fs::read_to_string(dev_path.join("device")).unwrap_or_default();
            let subsystem = std::fs::read_to_string(dev_path.join("subsystem_device")).unwrap_or_default();
            let mem_total = std::fs::read_to_string(dev_path.join("mem_info_vram_total")).unwrap_or_default();
            let mem_used = std::fs::read_to_string(dev_path.join("mem_info_vram_used")).unwrap_or_default();
            let temp = std::fs::read_to_string(dev_path.join("hwmon/hwmon0/temp1_input")).unwrap_or_default();
            let gfx = std::fs::read_to_string(dev_path.join("pp_dpm_sclk")).unwrap_or_default();

            gpus.push(GpuInfo {
                index: gpus.len() as u32,
                market_name: format!("AMD GPU {}", name),
                vendor_id: u32::from_str_radix(vendor.trim().trim_start_matches("0x"), 16).unwrap_or(0x1002),
                vendor_name: "AMD".into(),
                device_id: u64::from_str_radix(device.trim().trim_start_matches("0x"), 16).unwrap_or(0),
                subsystem_id: u32::from_str_radix(subsystem.trim().trim_start_matches("0x"), 16).unwrap_or(0),
                rev_id: 0, asic_serial: String::new(), num_cu: 0,
                gfx_version: "unknown".into(), vram_type: "unknown".into(),
                vram_total_mb: mem_total.trim().parse::<u64>().map(|b| (b/1024/1024) as u32).unwrap_or(0),
                vram_used_mb: mem_used.trim().parse::<u64>().map(|b| (b/1024/1024) as u32).unwrap_or(0),
                vram_pinned_mb: 0,
                vram_vendor: String::new(), bdf: String::new(), pcie_width: 0, pcie_speed_gt: 0,
                driver_version: String::new(), vbios_version: String::new(),
                temp_edge_c: temp.trim().parse::<f32>().map(|v| v/1000.0).ok(),
                temp_hotspot_c: None, temp_vram_c: None,
                gfx_clock_mhz: None, mem_clock_mhz: None, gfx_util_percent: None, power_w: None, power_cap_w: None,
                backend: "sysfs".into(),
                processes: Vec::new(),
            });
        }
        if gpus.is_empty() { anyhow::bail!("no AMD drm cards in /sys/class/drm"); }
        Ok(gpus)
    }
}
