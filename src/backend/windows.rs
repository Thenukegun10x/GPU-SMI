#![cfg(target_os = "windows")]
// Empty on other platforms: this backend needs ADL/WMI/DXGI. Kept declared
// (linux.rs stays compilable everywhere) so `cargo test` on Windows still
// covers the cross-platform parsers; all uses are cfg(windows)-gated.

use crate::gpu::GpuInfo;
use super::Backend;
use std::process::Command;
use std::ffi::c_void;
use std::os::raw::{c_char, c_int};
use std::alloc::{alloc, Layout};

const ADL_OK: i32 = 0;
// SECURITY: DLL sideload guard. Windows resolves a bare name via app-dir first,
// so a planted DLL next to this portable exe would win. Always try the absolute
// System32/driver path first; only fall back to bare name for Wine or exotic
// driver layouts.
fn load_lib_secure(bare: &str) -> Result<libloading::Library, libloading::Error> {
    unsafe {
        let abs = format!("C:\\Windows\\System32\\{}", bare);
        if let Ok(l) = libloading::Library::new(&abs) { return Ok(l); }
        libloading::Library::new(bare)
    }
}
fn load_adl_secure() -> Result<libloading::Library, libloading::Error> {
    unsafe {
        for p in [
            "C:\\Windows\\System32\\atiadlxx.dll",
            "C:\\Windows\\SysWOW64\\atiadlxx.dll",
            "C:\\Windows\\System32\\atiadlxy.dll",
        ] {
            if let Ok(l) = libloading::Library::new(p) { return Ok(l); }
        }
        libloading::Library::new("atiadlxx.dll")
            .or_else(|_| libloading::Library::new("atiadlxy.dll"))
    }
}
#[repr(C)] #[derive(Clone, Copy)] struct AdapterInfo { iSize: c_int, iAdapterIndex: c_int, strUDID: [c_char; 256], iBusNumber: c_int, iDeviceNumber: c_int, iFunctionNumber: c_int, iVendorID: c_int, strAdapterName: [c_char; 256], strDisplayName: [c_char; 256], iPresent: c_int, iExist: c_int, strDriverPath: [c_char; 256], strDriverPathExt: [c_char; 256], strPNPString: [c_char; 256], iOSDisplayIndex: c_int, }
impl Default for AdapterInfo { fn default() -> Self { unsafe { std::mem::zeroed() } } }
#[repr(C)] #[derive(Clone, Copy)] struct ADLPMLogSupportInfo { usSensors: [u16; 256], ulReserved: [i32; 16] }
#[repr(C)] #[derive(Clone, Copy)] struct ADLPMLogStartInput { usSensors: [u16; 256], ulSampleRate: u32, ulReserved: [i32; 15] }
#[repr(C)] #[derive(Clone, Copy)] struct ADLPMLogData { ulVersion: u32, ulActiveSampleRate: u32, ulLastUpdated: u64, ulValues: [[u32; 2]; 256], ulReserved: [u32; 256], }
#[repr(C)] #[derive(Clone, Copy)] struct ADLPMLogStartOutput { pLoggingAddress: *mut c_void, ulReserved: [i32; 14] }
#[repr(C)] #[derive(Clone, Copy)] struct ADLSingleSensorData { supported: c_int, value: c_int, }
#[repr(C)] #[derive(Clone, Copy)] struct ADLPMLogDataOutput { size: c_int, sensors: [ADLSingleSensorData; 256], }
impl Default for ADLPMLogSupportInfo { fn default() -> Self { unsafe { std::mem::zeroed() } } }
impl Default for ADLPMLogStartInput { fn default() -> Self { unsafe { std::mem::zeroed() } } }
impl Default for ADLPMLogStartOutput { fn default() -> Self { unsafe { std::mem::zeroed() } } }
impl Default for ADLPMLogDataOutput { fn default() -> Self { unsafe { std::mem::zeroed() } } }
// True hardware VRAM type via ADL memory info (no inference table)
#[repr(C)] #[derive(Clone, Copy)] struct ADLMemoryInfo { iMemorySize: i64, strMemoryType: [c_char; 256], iMemoryBandwidth: i64, }
impl Default for ADLMemoryInfo { fn default() -> Self { unsafe { std::mem::zeroed() } } }
#[repr(C)] #[derive(Clone, Copy)] struct ADLMemoryInfoX4 { iMemorySize: i64, strMemoryType: [c_char; 256], iMemoryBandwidth: i64, iHyperMemorySize: i64, iInvisibleMemorySize: i64, iVisibleMemorySize: i64, iVramVendorRevId: i64, iMemoryBandwidthX2: i64, iMemoryBitRateX2: i64, }
impl Default for ADLMemoryInfoX4 { fn default() -> Self { unsafe { std::mem::zeroed() } } }
unsafe extern "system" fn adl_malloc(size: c_int) -> *mut c_void { if size<=0 { return std::ptr::null_mut(); } let layout = Layout::from_size_align(size as usize, 8).unwrap_or(Layout::from_size_align(8,8).unwrap()); alloc(layout) as *mut c_void }

fn adl_query_vram_type(lib: &libloading::Library, ctx: *mut c_void, adapter_idx: c_int, use_adl2: bool) -> Option<String> {
    use libloading::Symbol;
    unsafe {
        let try_x4 = |ctx, idx| -> Option<String> {
            if let Ok(f) = lib.get::<unsafe extern "system" fn(*mut c_void, c_int, *mut ADLMemoryInfoX4)->c_int>(b"ADL2_Adapter_MemoryInfoX4_Get") {
                let mut info = ADLMemoryInfoX4::default();
                if f(ctx, idx, &mut info) == ADL_OK { let s=cstr_to_string(&info.strMemoryType); if !s.is_empty() && s.to_lowercase()!="unknown" { return Some(s); } }
            }
            if let Ok(f) = lib.get::<unsafe extern "system" fn(c_int, *mut ADLMemoryInfoX4)->c_int>(b"ADL_Adapter_MemoryInfoX4_Get") {
                let mut info = ADLMemoryInfoX4::default();
                if f(idx, &mut info) == ADL_OK { let s=cstr_to_string(&info.strMemoryType); if !s.is_empty() { return Some(s); } }
            }
            None
        };
        if let Some(s)=try_x4(ctx, adapter_idx) { return Some(s); }
        if use_adl2 {
            if let Ok(f) = lib.get::<unsafe extern "system" fn(*mut c_void, c_int, *mut ADLMemoryInfo)->c_int>(b"ADL2_Adapter_MemoryInfo_Get") {
                let mut info = ADLMemoryInfo::default();
                if f(ctx, adapter_idx, &mut info)==ADL_OK { let s=cstr_to_string(&info.strMemoryType); if !s.is_empty() { return Some(s); } }
            }
        }
        if let Ok(f) = lib.get::<unsafe extern "system" fn(c_int, *mut ADLMemoryInfo)->c_int>(b"ADL_Adapter_MemoryInfo_Get") {
            let mut info = ADLMemoryInfo::default();
            if f(adapter_idx, &mut info)==ADL_OK { let s=cstr_to_string(&info.strMemoryType); if !s.is_empty() { return Some(s); } }
        }
        if let Ok(f) = lib.get::<unsafe extern "system" fn(*mut c_void, c_int, *mut ADLMemoryInfo)->c_int>(b"ADL2_Adapter_MemoryInfo2_Get") {
            let mut info = ADLMemoryInfo::default();
            if f(ctx, adapter_idx, &mut info)==ADL_OK { let s=cstr_to_string(&info.strMemoryType); if !s.is_empty() { return Some(s); } }
        }
        None
    }
}

pub struct WindowsAdlBackend;
impl Backend for WindowsAdlBackend { fn discover(&self) -> anyhow::Result<Vec<GpuInfo>> { let mut gpus=Vec::new(); enrich_from_adl(&mut gpus)?; if gpus.is_empty(){ anyhow::bail!("ADL no data"); } Ok(gpus) } }

pub fn enrich_with_adl(gpus: &mut Vec<GpuInfo>) {
    if let Err(e) = enrich_from_adl(gpus) { eprintln!("[adl] PMLog enrich failed: {:#}", e); let _ = enrich_overdrive_n(gpus); }
}

fn enrich_from_adl(gpus: &mut Vec<GpuInfo>) -> anyhow::Result<()> {
    use libloading::Symbol;
    let lib = load_adl_secure()?;

    unsafe {
        // Try ADL2_Main_Control_Create for modern PMLog (Overdrive8) path
        let mut ctx: *mut c_void = std::ptr::null_mut();
        let mut use_adl2_ctx = false;
        let mut destroy_adl2: Option<Symbol<unsafe extern "system" fn(*mut c_void)->c_int>> = None;
        // Attempt ADL2 init
        if let Ok(adl2_create) = lib.get::<unsafe extern "system" fn(unsafe extern "system" fn(c_int)->*mut c_void, c_int, *mut *mut c_void)->c_int>(b"ADL2_Main_Control_Create") {
            let mut tmp: *mut c_void = std::ptr::null_mut();
            if adl2_create(adl_malloc, 1, &mut tmp) == ADL_OK { ctx = tmp; use_adl2_ctx = true; destroy_adl2 = lib.get::<unsafe extern "system" fn(*mut c_void)->c_int>(b"ADL2_Main_Control_Destroy").ok(); }
        }
        let (main_create_old, main_destroy_old) = if !use_adl2_ctx {
            let mc = lib.get::<unsafe extern "system" fn(unsafe extern "system" fn(c_int)->*mut c_void, c_int)->c_int>(b"ADL_Main_Control_Create")?;
            let md = lib.get::<unsafe extern "system" fn()->c_int>(b"ADL_Main_Control_Destroy")?;
            if mc(adl_malloc, 1) != ADL_OK { anyhow::bail!("ADL_Main_Control_Create failed"); }
            (Some(mc), Some(md))
        } else { (None, None) };

        // Get adapter infos via whichever API available
        let mut n = 0;
        let infos: Vec<AdapterInfo> = if use_adl2_ctx {
            if let Ok(f) = lib.get::<unsafe extern "system" fn(*mut c_void, *mut c_int)->c_int>(b"ADL2_Adapter_NumberOfAdapters_Get") {
                if f(ctx, &mut n) != ADL_OK { n=0; }
            }
            if n<=0 { // fallback to old
                if let Ok(f)=lib.get::<unsafe extern "system" fn(*mut c_int)->c_int>(b"ADL_Adapter_NumberOfAdapters_Get") { let _ = f(&mut n); }
            }
            let mut v = vec![AdapterInfo::default(); n as usize];
            for i in &mut v { i.iSize = std::mem::size_of::<AdapterInfo>() as i32; }
            if let Ok(f)=lib.get::<unsafe extern "system" fn(*mut c_void, *mut AdapterInfo, c_int)->c_int>(b"ADL2_Adapter_AdapterInfo_Get") {
                let sz = std::mem::size_of::<AdapterInfo>() as i32 * n;
                if f(ctx, v.as_mut_ptr(), sz) != ADL_OK { // fallback
                    if let Ok(f2)=lib.get::<unsafe extern "system" fn(*mut AdapterInfo, c_int)->c_int>(b"ADL_Adapter_AdapterInfo_Get") { let _=f2(v.as_mut_ptr(), sz); }
                }
            } else if let Ok(f2)=lib.get::<unsafe extern "system" fn(*mut AdapterInfo, c_int)->c_int>(b"ADL_Adapter_AdapterInfo_Get") {
                let sz = std::mem::size_of::<AdapterInfo>() as i32 * n;
                let _=f2(v.as_mut_ptr(), sz);
            }
            v
        } else {
            let f = lib.get::<unsafe extern "system" fn(*mut c_int)->c_int>(b"ADL_Adapter_NumberOfAdapters_Get")?;
            if f(&mut n) != ADL_OK { n=0; }
            let mut v = vec![AdapterInfo::default(); n as usize];
            for i in &mut v { i.iSize = std::mem::size_of::<AdapterInfo>() as i32; }
            let sz = std::mem::size_of::<AdapterInfo>() as i32 * n;
            if let Ok(f2)=lib.get::<unsafe extern "system" fn(*mut AdapterInfo, c_int)->c_int>(b"ADL_Adapter_AdapterInfo_Get") { let _=f2(v.as_mut_ptr(), sz); }
            v
        };

        if infos.is_empty() {
            if use_adl2_ctx { if let Some(d)=destroy_adl2 { let _=d(ctx); } } else { if let Some(d)=main_destroy_old { let _=d(); } }
            anyhow::bail!("no adapters");
        }
        let mut seen_bus = std::collections::HashSet::new();
        let mut dedup_infos: Vec<&AdapterInfo> = Vec::new();
        for inf in &infos {
            if inf.iVendorID != 0x1002 && inf.iVendorID != 1002 && inf.iVendorID != 0x3EA { continue; }
            if inf.iBusNumber<0 { continue; }
            if seen_bus.contains(&inf.iBusNumber) { continue; }
            seen_bus.insert(inf.iBusNumber);
            dedup_infos.push(inf);
        }

        // Try NEW PMLog API: ADL2_New_QueryPMLogData_Get (simplest, no device creation)
        let mut adl_entries: Vec<(i32, std::collections::HashMap<u32,u32>)> = Vec::new();
        if let Ok(new_query) = lib.get::<unsafe extern "system" fn(*mut c_void, c_int, *mut ADLPMLogDataOutput)->c_int>(b"ADL2_New_QueryPMLogData_Get") {
            for info in &dedup_infos {
                let mut out = ADLPMLogDataOutput::default();
                out.size = std::mem::size_of::<ADLPMLogDataOutput>() as i32;
                let ret = if use_adl2_ctx { new_query(ctx, info.iAdapterIndex, &mut out) } else { new_query(std::ptr::null_mut(), info.iAdapterIndex, &mut out) };
                if ret != ADL_OK { eprintln!("[adl] New_QueryPMLog adapter {} ret {}", info.iAdapterIndex, ret); continue; }
                let mut map = std::collections::HashMap::new();
                for (idx, s) in out.sensors.iter().enumerate() {
                    if s.supported != 0 { map.insert(idx as u32, s.value as u32); }
                }
                if !map.is_empty() { adl_entries.push((info.iAdapterIndex, map)); }
                else { eprintln!("[adl] New_QueryPMLog adapter {} empty", info.iAdapterIndex); }
            }
        }

        // If New_Query gave nothing, try legacy PMLog Start/Stop path
        if adl_entries.is_empty() {
            let pm_support_get: Result<Symbol<unsafe extern "system" fn(*mut c_void, c_int, *mut ADLPMLogSupportInfo)->c_int>,_> = lib.get(b"ADL2_Adapter_PMLog_Support_Get");
            let pm_start: Result<Symbol<unsafe extern "system" fn(*mut c_void, c_int, *mut ADLPMLogStartInput, *mut ADLPMLogStartOutput, *mut c_void)->c_int>,_> = lib.get(b"ADL2_Adapter_PMLog_Start");
            let pm_stop: Result<Symbol<unsafe extern "system" fn(*mut c_void, c_int, *mut c_void)->c_int>,_> = lib.get(b"ADL2_Adapter_PMLog_Stop");
            let dev_create: Result<Symbol<unsafe extern "system" fn(*mut c_void, c_int, *mut *mut c_void)->c_int>,_> = lib.get(b"ADL2_Device_PMLog_Device_Create");
            let dev_destroy: Result<Symbol<unsafe extern "system" fn(*mut c_void, *mut c_void)->c_int>,_> = lib.get(b"ADL2_Device_PMLog_Device_Destroy");
            if let (Ok(sg), Ok(st), Ok(sp), Ok(dc), Ok(dd)) = (pm_support_get, pm_start, pm_stop, dev_create, dev_destroy) {
                for info in &dedup_infos {
                    let mut support = ADLPMLogSupportInfo::default();
                    let r = sg(ctx, info.iAdapterIndex, &mut support);
                    if r != ADL_OK { eprintln!("[adl] PMLog Support_Get adapter {} ret {}", info.iAdapterIndex, r); continue; }
                    let mut start_in = ADLPMLogStartInput::default();
                    let mut cnt=0usize;
                    for &s in &support.usSensors { if s==0 { break; } if cnt<255 { start_in.usSensors[cnt]=s; cnt+=1; } }
                    if cnt==0 { eprintln!("[adl] adapter {} no sensors", info.iAdapterIndex); continue; }
                    start_in.usSensors[cnt]=0; start_in.ulSampleRate=100;
                    let mut hdev: *mut c_void = std::ptr::null_mut();
                    if dc(ctx, info.iAdapterIndex, &mut hdev) != ADL_OK { eprintln!("[adl] Device_Create failed {}", info.iAdapterIndex); continue; }
                    let mut out = ADLPMLogStartOutput::default();
                    let ret = st(ctx, info.iAdapterIndex, &mut start_in, &mut out, hdev);
                    if ret != ADL_OK { eprintln!("[adl] PMLog_Start adapter {} ret {}", info.iAdapterIndex, ret); let _=dd(ctx, hdev); continue; }
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let pm_data = out.pLoggingAddress as *const ADLPMLogData;
                    let mut map = std::collections::HashMap::new();
                    if !pm_data.is_null() {
                        let data=&*pm_data;
                        for entry in &data.ulValues { if entry[0]==0{break;} map.insert(entry[0], entry[1]); }
                    }
                    let _=sp(ctx, info.iAdapterIndex, hdev);
                    let _=dd(ctx, hdev);
                    if !map.is_empty() { adl_entries.push((info.iAdapterIndex, map)); }
                    else { eprintln!("[adl] PMLog data empty adapter {}", info.iAdapterIndex); }
                }
            } else { eprintln!("[adl] legacy PMLog symbols missing"); }
        }

        // Cleanup ADL
        if use_adl2_ctx { if let Some(d)=destroy_adl2 { let _=d(ctx); } } else { if let Some(d)=main_destroy_old { let _=d(); } }

        if adl_entries.is_empty() { anyhow::bail!("no PMLog data"); }

        if gpus.is_empty() {
            for (idx, entry) in adl_entries.iter().enumerate() {
                let map = &entry.1;
                let info = infos.iter().find(|i| i.iAdapterIndex==entry.0).unwrap();
                let dev = extract_dev_from_udid(&info.strUDID);
                let temp_edge = map.get(&8).copied();
                let temp_mem = map.get(&9).copied();
                let temp_hotspot = map.get(&27).copied();
                let gfxclk = map.get(&1).copied();
                let memclk = map.get(&2).copied();
                let gfx_act = map.get(&19).copied();
                let gfx_power = map.get(&30).copied();
                let asic_power = map.get(&23).copied();
                let board_power = map.get(&73).copied();
                let bus_speed = map.get(&40).copied();
                let bus_lanes = map.get(&41).copied();
                let vt = adl_query_vram_type(&lib, ctx, entry.0, use_adl2_ctx).unwrap_or_else(|| vram_type_for_dev(dev.unwrap_or(0)));
                gpus.push(GpuInfo{ index: idx as u32, market_name: cstr_to_string(&info.strAdapterName), vendor_id: 0x1002, vendor_name: "AMD".into(), device_id: dev.unwrap_or(0) as u64, subsystem_id: 0, rev_id: 0, asic_serial: String::new(), num_cu: 0, gfx_version: guess_gfx(dev.unwrap_or(0)), vram_type: vt, vram_total_mb: 0, vram_used_mb: 0, vram_pinned_mb: 0, vram_vendor: String::new(), bdf: format!("{} bus {} dev {} fn {}", cstr_to_string(&info.strUDID), info.iBusNumber, info.iDeviceNumber, info.iFunctionNumber), pcie_width: bus_lanes.unwrap_or(0) as u16, pcie_speed_gt: bus_speed.unwrap_or(0)/1000, driver_version: cstr_to_string(&info.strDriverPath), vbios_version: String::new(), temp_edge_c: temp_edge.map(|v| v as f32), temp_hotspot_c: temp_hotspot.map(|v| v as f32), temp_vram_c: temp_mem.map(|v| v as f32), gfx_clock_mhz: gfxclk, mem_clock_mhz: memclk, gfx_util_percent: gfx_act, power_w: board_power.or(asic_power).or(gfx_power).map(|v| v as f32/10.0), power_cap_w: None, backend: "adl-pmlog".into(), processes: Vec::new(), });
            }
        } else {
            for (i, gpu) in gpus.iter_mut().enumerate() {
                let (adapter_idx, map) = if let Some((adi,m))=adl_entries.get(i) { (*adi, m) } else if let Some((adi,m))=adl_entries.iter().find(|(adi,_)| { if let Some(w)=infos.iter().find(|inf| inf.iAdapterIndex==*adi) { if let Some(dev)=extract_dev_from_udid(&w.strUDID) { dev as u64==gpu.device_id } else { false } } else { false } }) { (*adi, m) } else { continue; };
                if let Some(&v)=map.get(&8) { gpu.temp_edge_c=Some(v as f32); }
                if let Some(&v)=map.get(&27) { gpu.temp_hotspot_c=Some(v as f32); }
                if let Some(&v)=map.get(&9) { gpu.temp_vram_c=Some(v as f32); }
                if let Some(&v)=map.get(&1) { gpu.gfx_clock_mhz=Some(v); }
                if let Some(&v)=map.get(&2) { gpu.mem_clock_mhz=Some(v); }
                if let Some(&v)=map.get(&19) { gpu.gfx_util_percent=Some(v); }
                let p = map.get(&73).or(map.get(&23)).or(map.get(&30)).copied();
                if let Some(v)=p { gpu.power_w=Some(v as f32/10.0); }
                if let Some(&v)=map.get(&41) { gpu.pcie_width=v as u16; }
                if let Some(&v)=map.get(&40) { gpu.pcie_speed_gt=v/1000; }
                // True hardware VRAM type (overwrites inference)
                if let Some(vt) = adl_query_vram_type(&lib, ctx, adapter_idx, use_adl2_ctx) {
                    let norm = vt.trim().to_string();
                    if norm.to_lowercase() != "unknown" && !norm.is_empty() && norm.to_lowercase() != "n/a" { gpu.vram_type = norm; }
                }
                if !gpu.backend.contains("adl") { gpu.backend=format!("{}+adl", gpu.backend); }
            }
        }
        Ok(())
    }
}

fn enrich_overdrive_n(gpus: &mut Vec<GpuInfo>) -> anyhow::Result<()> {
    use libloading::Symbol;
    let lib = load_adl_secure()?;
    unsafe {
        let main_create: Symbol<unsafe extern "system" fn(unsafe extern "system" fn(c_int)->*mut c_void, c_int)->c_int> = lib.get(b"ADL_Main_Control_Create")?;
        let main_destroy: Symbol<unsafe extern "system" fn()->c_int> = lib.get(b"ADL_Main_Control_Destroy")?;
        let temp_get: Symbol<unsafe extern "system" fn(*mut c_void, c_int, c_int, *mut c_int)->c_int> = lib.get(b"ADL2_OverdriveN_Temperature_Get")?;
        let clocks_get: Symbol<unsafe extern "system" fn(*mut c_void, c_int, *mut ADLODNPerformanceStatus)->c_int> = match lib.get(b"ADL2_OverdriveN_PerformanceStatus_Get") { Ok(s)=>s, Err(_)=> anyhow::bail!("OverdriveN not supported") };
        let ctx: *mut c_void = std::ptr::null_mut();
        if main_create(adl_malloc, 1) != ADL_OK { anyhow::bail!("ADL create failed"); }
        let num: Symbol<unsafe extern "system" fn(*mut c_int)->c_int> = lib.get(b"ADL_Adapter_NumberOfAdapters_Get")?;
        let info_get: Symbol<unsafe extern "system" fn(*mut AdapterInfo, c_int)->c_int> = lib.get(b"ADL_Adapter_AdapterInfo_Get")?;
        let mut n=0; num(&mut n);
        let mut infos = vec![AdapterInfo::default(); n as usize];
        for inf in &mut infos { inf.iSize = std::mem::size_of::<AdapterInfo>() as i32; }
        info_get(infos.as_mut_ptr(), std::mem::size_of::<AdapterInfo>() as i32 * n);
        for (i, gpu) in gpus.iter_mut().enumerate() {
            let ad_idx = if let Some(inf)=infos.iter().find(|x| (x.iVendorID==0x1002||x.iVendorID==1002||x.iVendorID==0x3EA) && extract_dev_from_udid(&x.strUDID).map(|d| d as u64==gpu.device_id).unwrap_or(false)) { inf.iAdapterIndex } else { infos.iter().filter(|x| x.iVendorID==0x1002||x.iVendorID==1002||x.iVendorID==0x3EA).nth(i).map(|x| x.iAdapterIndex).unwrap_or(i as i32) };
            let mut temp: c_int = 0;
            if temp_get(ctx, ad_idx, 0, &mut temp) == ADL_OK { gpu.temp_edge_c = Some(temp as f32 / 1000.0); }
            let mut status = ADLODNPerformanceStatus::default();
            if clocks_get(ctx, ad_idx, &mut status) == ADL_OK {
                if status.iGFXClock > 0 { gpu.gfx_clock_mhz = Some((status.iGFXClock / 100) as u32); }
                else if status.iCoreClock > 0 { gpu.gfx_clock_mhz = Some((status.iCoreClock / 100) as u32); }
                if status.iMemoryClock > 0 { gpu.mem_clock_mhz = Some((status.iMemoryClock / 100) as u32); }
                if status.iGPUActivityPercent >=0 && status.iGPUActivityPercent <=100 { gpu.gfx_util_percent = Some(status.iGPUActivityPercent as u32); }
                if status.iCurrentBusSpeed >0 { gpu.pcie_speed_gt = (status.iCurrentBusSpeed / 1000) as u32; }
                if status.iCurrentBusLanes >0 { gpu.pcie_width = status.iCurrentBusLanes as u16; }
            }
            if !gpu.backend.contains("adl") { gpu.backend = format!("{}+adl-odn", gpu.backend); }
        }
        main_destroy();
        Ok(())
    }
}

#[repr(C)] #[derive(Clone, Copy)] struct ADLODNPerformanceStatus { iCoreClock: c_int, iMemoryClock: c_int, iDCEFClock: c_int, iGFXClock: c_int, iUVDClock: c_int, iVCEClock: c_int, iGPUActivityPercent: c_int, iCurrentCorePerformanceLevel: c_int, iCurrentMemoryPerformanceLevel: c_int, iCurrentDCEFPerformanceLevel: c_int, iCurrentGFXPerformanceLevel: c_int, iUVDPerformanceLevel: c_int, iVCEPerformanceLevel: c_int, iCurrentBusSpeed: c_int, iCurrentBusLanes: c_int, iMaximumBusLanes: c_int, iVDDC: c_int, iVDDCI: c_int, }
impl Default for ADLODNPerformanceStatus { fn default() -> Self { unsafe { std::mem::zeroed() } } }

fn cstr_to_string(arr: &[c_char]) -> String { let bytes: Vec<u8> = arr.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect(); String::from_utf8_lossy(&bytes).trim().to_string() }
fn extract_dev_from_udid(arr: &[c_char]) -> Option<u32> { let s=cstr_to_string(arr); s.find("DEV_").and_then(|i| { let hex=&s[i+4..]; let end=hex.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(hex.len().min(4)); u32::from_str_radix(&hex[..end],16).ok() }) }

fn smbios_to_str(t: u32) -> &'static str {
    match t {
        12 => "SDRAM", 13 => "RDRAM", 19 => "DDR", 20 => "DDR2", 21 => "DDR2 FB-DIMM", 24 => "DDR3", 26 => "DDR4",
        30 => "LPDDR", 31 => "LPDDR2", 32 => "LPDDR3", 33 => "LPDDR4", 34 => "DDR5", 35 => "LPDDR5",
        _ => "Unknown",
    }
}
fn system_ram_type() -> Option<String> {
    // Query Win32_PhysicalMemory SMBIOSMemoryType - true hardware, not inference
    let ps = r#"(Get-CimInstance Win32_PhysicalMemory | Select-Object -First 1 -ExpandProperty SMBIOSMemoryType)"#;
    if let Ok(out) = Command::new("powershell").args(["-NoProfile","-Command", ps]).output() {
        if let Ok(s) = String::from_utf8(out.stdout) {
            if let Ok(v) = s.trim().parse::<u32>() { let t = smbios_to_str(v); if t!="Unknown" { return Some(t.to_string()); } }
        }
    }
    None
}
fn vram_type_for_dev(dev: u32) -> String {
    // iGPU/APU -> system RAM type (DDR4 vs DDR5 vs LPDDR4/5) via WMI - true hardware
    // Discrete -> VRAM type via PCI DEV table (covers DDR4/5, GDDR5/6, HBM)
    let is_igpu = matches!(dev, 0x164E|0x15E8|0x15BF|0x1586|0x1681|0x1636|0x15D8|0x9874|0x15DD|0x1718);
    if is_igpu {
        if let Some(t) = system_ram_type() { return format!("{} (Shared)", t); }
        // fallback by era
        return match dev {
            0x164E|0x15BF|0x1586|0x15E8 => "DDR5 (Shared)".into(), // RDNA3 iGPU era DDR5 only
            0x15DD|0x1636|0x1681 => "DDR4 (Shared)".into(), // older Raven/Picasso
            _ => "DDR4/DDR5 (Shared)".into(),
        };
    }
    // Discrete VRAM - true type by PCI DEV (hardware, not hardcode single)
    match dev {
        0x6863|0x6864|0x6867|0x686C|0x687F|0x6860|0x6861 => "HBM2".into(),
        0x7360|0x73A0|0x73AB => "HBM2e".into(),
        0x67DF|0x67EF|0x67FF|0x6FDF|0x699F|0x67C0|0x67E0 => "GDDR5".into(),
        _ => "GDDR6".into(),
    }
}
pub struct WmiBackend;
impl Backend for WmiBackend { fn discover(&self) -> anyhow::Result<Vec<GpuInfo>> { let ps = r#"Get-CimInstance Win32_VideoController | Where-Object { $_.PNPDeviceID -like 'PCI\VEN_1002*' } | Select-Object Name,PNPDeviceID,DriverVersion,AdapterRAM | ConvertTo-Json -Compress"#; let out = Command::new("powershell").args(["-NoProfile","-Command",ps]).output()?; let txt=String::from_utf8_lossy(&out.stdout).trim().to_string(); if txt.is_empty()||txt=="null"{anyhow::bail!("no AMD GPU via WMI");} let json_str=if txt.trim_start().starts_with('['){txt}else{format!("[{}]",txt)}; let vals:serde_json::Value=serde_json::from_str(&json_str)?; let mut gpus=Vec::new(); if let Some(arr)=vals.as_array(){ for(i,v) in arr.iter().enumerate(){ let name=v.get("Name").and_then(|x|x.as_str()).unwrap_or("AMD GPU").to_string(); let pnp=v.get("PNPDeviceID").and_then(|x|x.as_str()).unwrap_or(""); let dev=extract_hex(pnp,"DEV_"); let sub=extract_hex(pnp,"SUBSYS_"); let rev=extract_hex(pnp,"REV_"); let ram=v.get("AdapterRAM").and_then(|x|x.as_u64()).unwrap_or(0); let drv=v.get("DriverVersion").and_then(|x|x.as_str()).unwrap_or("").to_string(); let vt = registry_vram_type(i).unwrap_or_else(|| vram_type_for_dev(dev.unwrap_or(0))); gpus.push(GpuInfo{ index:i as u32, market_name:name, vendor_id:0x1002, vendor_name:"Advanced Micro Devices, Inc. [AMD/ATI]".into(), device_id:dev.unwrap_or(0) as u64, subsystem_id:sub.unwrap_or(0) as u32, rev_id:rev.unwrap_or(0) as u32, asic_serial:String::new(), num_cu:0, gfx_version:guess_gfx(dev.unwrap_or(0)), vram_type: vt, vram_total_mb:(ram/1024/1024) as u32, vram_used_mb:0, vram_pinned_mb:0, vram_vendor:String::new(), bdf:pnp.to_string(), pcie_width:0, pcie_speed_gt:0, driver_version:drv, vbios_version:String::new(), temp_edge_c:None, temp_hotspot_c:None, temp_vram_c:None, gfx_clock_mhz:None, mem_clock_mhz:None, gfx_util_percent:None, power_w:None, power_cap_w:None, backend:"wmi".into(), processes: Vec::new(), }); } } if gpus.is_empty(){anyhow::bail!("no AMD WMI entries");} Ok(gpus) } }
fn extract_hex(s:&str,key:&str)->Option<u32>{ s.find(key).and_then(|i|{ let hex=&s[i+key.len()..]; let end=hex.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(hex.len().min(8)); u32::from_str_radix(&hex[..end],16).ok() }) }
fn guess_gfx(dev_id:u32)->String{ crate::gpu::gfx_for_pci_dev(dev_id) }
fn registry_vram_type(_idx:usize)->Option<String>{None}

pub fn enrich_vram_usage(gpus: &mut Vec<GpuInfo>) {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
        Ok(f) => f,
        Err(_) => return,
    };
    let gdi = match load_lib_secure("gdi32.dll") {
        Ok(l) => l,
        Err(_) => return,
    };
    let query_stats = match unsafe { gdi.get::<unsafe extern "system" fn(*mut u8) -> i32>(b"D3DKMTQueryStatistics") } {
        Ok(f) => f,
        Err(_) => return,
    };

    let mut luid_map: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut i = 0;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(i) } {
        if let Ok(desc) = unsafe { adapter.GetDesc1() } {
            let luid_str = format!("0x{:08x}_0x{:08x}", desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart).to_lowercase();
            let dev_id = desc.DeviceId as u64;
            let gpu_idx = gpus.iter().position(|g| (g.device_id != 0 && g.device_id == dev_id) || (g.device_id == 0 && g.index == i));
            if let Some(idx) = gpu_idx {
                luid_map.insert(luid_str, idx);
                let gpu = &mut gpus[idx];
                let mut buf = [0u8; 0x340];
                buf[0] = 0; // Type = 0 (ADAPTER)
                buf[4..8].copy_from_slice(&desc.AdapterLuid.LowPart.to_ne_bytes());
                buf[8..12].copy_from_slice(&desc.AdapterLuid.HighPart.to_ne_bytes());
                let ret = unsafe { query_stats(buf.as_mut_ptr()) };
                if ret == 0 {
                    let nb_segments = u32::from_ne_bytes(buf[24..28].try_into().unwrap_or([0; 4]));
                    let mut total_resident: u64 = 0;
                    let mut total_committed: u64 = 0;
                    let mut total_limit: u64 = 0;
                    let mut has_dedicated = false;

                    for seg in 0..nb_segments {
                        let mut sbuf = [0u8; 0x340];
                        sbuf[0] = 3; // D3DKMT_QUERYSTATISTICS_SEGMENT
                        sbuf[4..8].copy_from_slice(&desc.AdapterLuid.LowPart.to_ne_bytes());
                        sbuf[8..12].copy_from_slice(&desc.AdapterLuid.HighPart.to_ne_bytes());
                        sbuf[0x320..0x324].copy_from_slice(&seg.to_ne_bytes());
                        let sret = unsafe { query_stats(sbuf.as_mut_ptr()) };
                        if sret == 0 {
                            let commit_limit = u64::from_ne_bytes(sbuf[24..32].try_into().unwrap_or([0; 8]));
                            let bytes_committed = u64::from_ne_bytes(sbuf[32..40].try_into().unwrap_or([0; 8]));
                            let bytes_resident = u64::from_ne_bytes(sbuf[40..48].try_into().unwrap_or([0; 8]));
                            let aperture = u32::from_ne_bytes(sbuf[64..68].try_into().unwrap_or([0; 4]));

                            if aperture == 0 && commit_limit > 0 {
                                has_dedicated = true;
                                total_resident += bytes_resident;
                                total_committed += bytes_committed;
                                total_limit += commit_limit;
                            }
                        }
                    }

                    if has_dedicated {
                        let used_mb = (total_resident / (1024 * 1024)) as u32;
                        let committed_mb = (total_committed / (1024 * 1024)) as u32;
                        gpu.vram_used_mb = used_mb;
                        if used_mb > committed_mb {
                            gpu.vram_pinned_mb = used_mb - committed_mb;
                        }
                        if gpu.vram_total_mb == 0 && total_limit > 0 {
                            gpu.vram_total_mb = (total_limit / (1024 * 1024)) as u32;
                        }
                    }
                }
            }
        }
        i += 1;
    }

    query_gpu_processes(&luid_map, gpus);
}

fn query_gpu_processes(luid_map: &std::collections::HashMap<String, usize>, gpus: &mut [GpuInfo]) {
    use std::collections::HashMap;
    let kernel32 = match load_lib_secure("kernel32.dll") {
        Ok(l) => l,
        Err(_) => return,
    };
    let pdh = match load_lib_secure("pdh.dll") {
        Ok(l) => l,
        Err(_) => return,
    };

    #[repr(C)]
    struct PROCESSENTRY32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }
    let create_snapshot = match unsafe { kernel32.get::<unsafe extern "system" fn(u32, u32) -> *mut c_void>(b"CreateToolhelp32Snapshot") } {
        Ok(f) => f,
        Err(_) => return,
    };
    let proc_first = match unsafe { kernel32.get::<unsafe extern "system" fn(*mut c_void, *mut PROCESSENTRY32W) -> i32>(b"Process32FirstW") } {
        Ok(f) => f,
        Err(_) => return,
    };
    let proc_next = match unsafe { kernel32.get::<unsafe extern "system" fn(*mut c_void, *mut PROCESSENTRY32W) -> i32>(b"Process32NextW") } {
        Ok(f) => f,
        Err(_) => return,
    };
    let close_handle = match unsafe { kernel32.get::<unsafe extern "system" fn(*mut c_void) -> i32>(b"CloseHandle") } {
        Ok(f) => f,
        Err(_) => return,
    };

    let mut pid_names: HashMap<u32, String> = HashMap::new();
    unsafe {
        let snap = create_snapshot(0x00000002, 0); // TH32CS_SNAPPROCESS = 2
        if !snap.is_null() && snap as isize != -1 {
            let mut pe = std::mem::zeroed::<PROCESSENTRY32W>();
            pe.dw_size = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if proc_first(snap, &mut pe) != 0 {
                loop {
                    let end = pe.sz_exe_file.iter().position(|&c| c == 0).unwrap_or(pe.sz_exe_file.len());
                    let name = String::from_utf16_lossy(&pe.sz_exe_file[..end]);
                    pid_names.insert(pe.th32_process_id, name);
                    if proc_next(snap, &mut pe) == 0 {
                        break;
                    }
                }
            }
            close_handle(snap);
        }
    }

    #[repr(C)]
    struct PDH_FMT_COUNTERVALUE {
        cstatus: u32,
        large_value: i64,
    }
    #[repr(C)]
    struct PDH_FMT_COUNTERVALUE_ITEM_W {
        sz_name: *mut u16,
        fmt_value: PDH_FMT_COUNTERVALUE,
    }

    let open_query = match unsafe { pdh.get::<unsafe extern "system" fn(*const u16, usize, *mut usize) -> i32>(b"PdhOpenQueryW") } {
        Ok(f) => f,
        Err(_) => return,
    };
    let add_counter = match unsafe { pdh.get::<unsafe extern "system" fn(usize, *const u16, usize, *mut usize) -> i32>(b"PdhAddEnglishCounterW") } {
        Ok(f) => f,
        Err(_) => return,
    };
    let collect_data = match unsafe { pdh.get::<unsafe extern "system" fn(usize) -> i32>(b"PdhCollectQueryData") } {
        Ok(f) => f,
        Err(_) => return,
    };
    let get_array = match unsafe { pdh.get::<unsafe extern "system" fn(usize, u32, *mut u32, *mut u32, *mut PDH_FMT_COUNTERVALUE_ITEM_W) -> i32>(b"PdhGetFormattedCounterArrayW") } {
        Ok(f) => f,
        Err(_) => return,
    };
    let close_query = match unsafe { pdh.get::<unsafe extern "system" fn(usize) -> i32>(b"PdhCloseQuery") } {
        Ok(f) => f,
        Err(_) => return,
    };

    let path_local: Vec<u16> = "\\GPU Process Memory(*)\\Local Usage\0".encode_utf16().collect();
    let path_shared: Vec<u16> = "\\GPU Process Memory(*)\\Shared Usage\0".encode_utf16().collect();

    let mut collect_counter_data = |counter_path: &[u16], is_shared: bool, gpus: &mut [GpuInfo]| {
        unsafe {
            let mut query: usize = 0;
            let mut counter: usize = 0;
            if open_query(std::ptr::null(), 0, &mut query) == 0 {
                if add_counter(query, counter_path.as_ptr(), 0, &mut counter) == 0 {
                    collect_data(query);
                    let mut buffer_size: u32 = 0;
                    let mut item_count: u32 = 0;
                    let _ = get_array(counter, 0x00000400, &mut buffer_size, &mut item_count, std::ptr::null_mut());
                    if buffer_size > 0 {
                        let mut buffer: Vec<u8> = vec![0u8; buffer_size as usize];
                        if get_array(counter, 0x00000400, &mut buffer_size, &mut item_count, buffer.as_mut_ptr() as *mut _) == 0 {
                            let items = std::slice::from_raw_parts(buffer.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W, item_count as usize);
                            for it in items {
                                if it.sz_name.is_null() { continue; }
                                let mut len = 0;
                                while *it.sz_name.add(len) != 0 { len += 1; }
                                let slice = std::slice::from_raw_parts(it.sz_name, len);
                                let name = String::from_utf16_lossy(slice);
                                let val_mb = (it.fmt_value.large_value / (1024 * 1024)) as u32;
                                if val_mb == 0 { continue; }

                                let name_lower = name.to_lowercase();
                                let pid: u32 = name_lower.strip_prefix("pid_")
                                    .and_then(|s| s.split('_').next())
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0);
                                if pid == 0 { continue; }

                                let target_gpu_idx = luid_map.iter().find_map(|(luid_pat, &idx)| {
                                    if name_lower.contains(luid_pat) { Some(idx) } else { None }
                                });

                                if let Some(gpu_idx) = target_gpu_idx {
                                    if let Some(gpu) = gpus.get_mut(gpu_idx) {
                                        // For discrete GPUs with dedicated VRAM, only use Local Usage.
                                        // Only consider Shared Usage for APUs/iGPUs (shared memory) if no local usage was recorded.
                                        if is_shared && !gpu.vram_type.contains("Shared") {
                                            continue;
                                        }
                                        if gpu.vram_total_mb > 0 && val_mb > gpu.vram_total_mb {
                                            continue;
                                        }
                                        let proc_name = pid_names.get(&pid).cloned().unwrap_or_else(|| "Unknown".to_string());
                                        let proc_type = crate::gpu::classify_proc_type(&proc_name);

                                        if let Some(existing) = gpu.processes.iter_mut().find(|p| p.pid == pid) {
                                            if val_mb > existing.mem_used_mb {
                                                existing.mem_used_mb = val_mb;
                                            }
                                        } else {
                                            gpu.processes.push(crate::gpu::ProcessInfo {
                                                pid,
                                                name: proc_name,
                                                mem_used_mb: val_mb,
                                                proc_type,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                close_query(query);
            }
        }
    };

    // First collect physical VRAM (Local Usage)
    collect_counter_data(&path_local, false, gpus);

    // If any APU / shared-memory GPU has no processes, query Shared Usage as fallback
    let needs_shared = gpus.iter().any(|g| g.vram_type.contains("Shared") && g.processes.is_empty());
    if needs_shared {
        collect_counter_data(&path_shared, true, gpus);
    }

    for gpu in gpus {
        gpu.processes.sort_by(|a, b| b.mem_used_mb.cmp(&a.mem_used_mb));
    }
}
