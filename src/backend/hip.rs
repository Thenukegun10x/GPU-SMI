use crate::gpu::GpuInfo;
use super::Backend;
use libloading::{Library, Symbol};
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct HipUuid { bytes: [u8; 16] }
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct HipDeviceProp {
    name: [c_char; 256],
    uuid: HipUuid,
    luid: [c_char; 8],
    luidDeviceNodeMask: u32,
    // 4 bytes padding on 64-bit before size_t
    _pad_align: u32,
    totalGlobalMem: usize,
    sharedMemPerBlock: usize,
    regsPerBlock: c_int,
    warpSize: c_int,
    memPitch: usize,
    maxThreadsPerBlock: c_int,
    _pad: [u8; 2048],
}

pub struct HipBackend;

impl Backend for HipBackend {
    fn discover(&self) -> anyhow::Result<Vec<GpuInfo>> {
        // Try TheRock path first, then system PATH
        let candidates = [
            r"C:\TheRock\build\bin\amdhip64_7.dll",
            r"C:\TheRock\build\bin\amdhip64.dll",
            "amdhip64_7.dll",
            "amdhip64.dll",
            "libamdhip64.so",
            "libamdhip64.so.7",
        ];
        let mut lib: Option<Library> = None;
        for p in candidates {
            unsafe { if let Ok(l) = Library::new(p) { lib = Some(l); break; } }
        }
        let lib = lib.ok_or_else(|| anyhow::anyhow!("HIP runtime not found (amdhip64)"))?;

        unsafe {
            let hipGetDeviceCount: Symbol<unsafe extern "C" fn(*mut c_int) -> c_int> = lib.get(b"hipGetDeviceCount")?;
            let hipGetDeviceProperties: Symbol<unsafe extern "C" fn(*mut HipDeviceProp, c_int) -> c_int> = lib.get(b"hipGetDevicePropertiesR0600")
                .or_else(|_| lib.get(b"hipGetDeviceProperties"))?;

            let mut count: c_int = 0;
            let ret = hipGetDeviceCount(&mut count);
            if ret != 0 { anyhow::bail!("hipGetDeviceCount failed: {}", ret); }

            let mut gpus = Vec::new();
            for i in 0..count {
                let mut prop = std::mem::zeroed::<HipDeviceProp>();
                let ret = hipGetDeviceProperties(&mut prop, i);
                if ret != 0 { continue; }
                let name = CStr::from_ptr(prop.name.as_ptr()).to_string_lossy().to_string();
                let vram_mb = (prop.totalGlobalMem / (1024*1024)) as u32;
                // Try to map name to gfx version via simple table (TheRock gfx1200/1201 = RDNA4)
                let gfx = if name.contains("780M") || name.contains("8060S") || name.contains("Radeon(TM) Graphics") { "gfx1151" }
                    else if name.contains("7900") || name.contains("RX 7900") { "gfx1100" }
                    else if name.contains("9070") || name.contains("RX 90") { "gfx1201" }
                    else { "unknown" };
                // VRAM type: iGPU uses system RAM (DDR4/DDR5/LPDDR4/5) autodetected via WMI when possible, dGPU GDDR/HBM via DEV table
                let vram_type = if gfx=="gfx1151" || name.contains("Radeon(TM) Graphics") || name.contains("780M") || name.contains("860M") || name.contains("880M") {
                    #[cfg(target_os="windows")]
                    {
                        // True hardware: query Win32_PhysicalMemory SMBIOS
                        let ps = r#"(Get-CimInstance Win32_PhysicalMemory | Select-Object -First 1 -ExpandProperty SMBIOSMemoryType)"#;
                        let smbios = std::process::Command::new("powershell").args(["-NoProfile","-Command", ps]).output()
                            .ok().and_then(|o| String::from_utf8(o.stdout).ok()).and_then(|s| s.trim().parse::<u32>().ok());
                        let t = match smbios { Some(26) => "DDR4", Some(34) => "DDR5", Some(32) => "LPDDR3", Some(33) => "LPDDR4", Some(35) => "LPDDR5", _ => "DDR5" };
                        format!("{} (Shared)", t)
                    }
                    #[cfg(not(target_os="windows"))] { "DDR4/DDR5 (Shared)".to_string() }
                } else if name.contains("Vega") && (name.contains("Graphics") || name.contains("8") || name.contains("11")) {
                    "DDR4 (Shared)".to_string()
                } else { "GDDR6".to_string() };
                gpus.push(GpuInfo {
                    index: i as u32,
                    market_name: name.clone(),
                    vendor_id: 0x1002,
                    vendor_name: "Advanced Micro Devices, Inc. [AMD/ATI]".into(),
                    device_id: 0,
                    subsystem_id: 0,
                    rev_id: 0,
                    asic_serial: String::new(),
                    num_cu: if prop.warpSize != 0 { (prop.maxThreadsPerBlock / prop.warpSize) as u32 } else { 0 },
                    gfx_version: gfx.into(),
                    vram_type: vram_type.into(),
                    vram_total_mb: vram_mb,
                    vram_used_mb: 0,
                    vram_vendor: String::new(),
                    bdf: String::new(),
                    pcie_width: 0,
                    pcie_speed_gt: 0,
                    driver_version: String::new(),
                    vbios_version: String::new(),
                    temp_edge_c: None,
                    temp_hotspot_c: None,
                    temp_vram_c: None,
                    gfx_clock_mhz: None,
                    mem_clock_mhz: None,
                    gfx_util_percent: None,
                    power_w: None,
                    power_cap_w: None,
                    backend: "hip".into(),
                });
            }
            Ok(gpus)
        }
    }
}
