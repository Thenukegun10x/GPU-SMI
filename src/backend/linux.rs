#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
// Parsers stay compiled on all targets so `cargo test` on Windows covers
// them; sysfs-touching helpers are Linux-only at runtime (called from
// mod.rs under cfg(target_os = "linux")) — hence the allow(dead_code).

use crate::gpu::{GpuInfo, ProcessInfo};
use super::Backend;
use libloading::{Library, Symbol};
use std::path::{Path, PathBuf};

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
            let _get_socket_handles: Symbol<unsafe extern "C" fn(*mut u32, *mut *mut std::ffi::c_void) -> i32> = lib.get(b"amdsmi_get_socket_handles")?;

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

// ---------------- sysfs helpers (pure parts unit-tested below) ----------------

fn read_trim(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn parse_hex_u32(s: &str) -> Option<u32> {
    let t = s.trim();
    let h = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    u32::from_str_radix(h, 16).ok()
}

fn read_hex(path: PathBuf) -> Option<u32> {
    read_trim(&path).and_then(|s| parse_hex_u32(&s))
}

fn read_u64(path: &Path) -> Option<u64> {
    read_trim(path)?.parse::<u64>().ok()
}

/// Only exact `cardN` nodes. Connectors (`card0-DP-1`), `renderD*` and
/// `controlD*` must not become phantom GPUs (their `device` link resolves
/// to the same PCI device and would duplicate the card).
fn is_drm_card_name(name: &str) -> bool {
    match name.strip_prefix("card") {
        Some(rest) => !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

/// `(card_name, device_path)` for every AMD card, PCI order. `device` is the
/// `/sys/class/drm/cardN/device` symlink into `/sys/bus/pci/devices/<bdf>`.
fn amd_cards() -> Vec<(String, PathBuf)> {
    let mut out: Vec<(u32, String, PathBuf)> = Vec::new();
    let entries = match std::fs::read_dir("/sys/class/drm") {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };
    for e in entries.filter_map(|e| e.ok()) {
        let name = e.file_name().to_string_lossy().into_owned();
        if !is_drm_card_name(&name) {
            continue;
        }
        let idx: u32 = name[4..].parse().unwrap_or(u32::MAX);
        let dev = e.path().join("device");
        if !dev.exists() {
            continue;
        }
        if read_hex(dev.join("vendor")) != Some(0x1002) {
            continue;
        }
        out.push((idx, name, dev));
    }
    out.sort_by_key(|(i, _, _)| *i);
    out.into_iter().map(|(_, n, d)| (n, d)).collect()
}

/// hwmon dir of the amdgpu sensors (`temp1_input`, `power1_average`,
/// `freq1_input`, ...). Falls back to the first hwmon node if the driver
/// didn't set `name` (very old kernels).
fn amdgpu_hwmon(dev: &Path) -> Option<PathBuf> {
    let dir = dev.join("hwmon");
    let mut first: Option<PathBuf> = None;
    for e in std::fs::read_dir(dir).ok()? {
        let p = e.ok()?.path();
        if first.is_none() {
            first = Some(p.clone());
        }
        if read_trim(&p.join("name")).as_deref() == Some("amdgpu") {
            return Some(p);
        }
    }
    first
}

/// Active DPM level, e.g. `"0: 615Mhz\n1: 800Mhz *\n"` -> 800.
/// Deep-sleep levels look like `"S: 19Mhz *"` and are handled too.
fn parse_pp_dpm_active(text: &str) -> Option<u32> {
    for line in text.lines() {
        let (_, right) = match line.split_once(':') {
            Some(x) => x,
            None => continue,
        };
        if !right.contains('*') {
            continue;
        }
        return parse_freq_mhz(&right.replace('*', ""));
    }
    None
}

/// `"800Mhz"` -> 800, `"3.4Ghz"` -> 3400. pp_dpm files use Mhz; hwmon
/// `freqN_input` is handled separately (Hz, see caller).
fn parse_freq_mhz(s: &str) -> Option<u32> {
    let t = s.trim();
    if let Some(v) = t.strip_suffix("Mhz").or_else(|| t.strip_suffix("MHz")).or_else(|| t.strip_suffix("mhz")) {
        return v.trim().parse::<u32>().ok();
    }
    if let Some(v) = t.strip_suffix("Ghz").or_else(|| t.strip_suffix("GHz")).or_else(|| t.strip_suffix("ghz")) {
        return v.trim().parse::<f64>().ok().map(|g| (g * 1000.0).round() as u32);
    }
    t.parse::<u32>().ok()
}

/// `"16 GT/s PCIe"` -> 16. PCI `current_link_speed` format, GT/s.
fn parse_link_speed_gts(s: &str) -> Option<u32> {
    s.split_whitespace().next()?.parse::<f64>().ok().map(|v| v.round() as u32)
}

fn linux_vram_type(dev: u32) -> String {
    // APUs share system RAM; sysfs exposes no DIMM-type file, so report honestly.
    const APUS: &[u32] = &[0x164E, 0x15E8, 0x15BF, 0x1586, 0x1681, 0x1636, 0x15D8, 0x9874, 0x15DD, 0x1718];
    if APUS.contains(&dev) {
        return "DDR4/DDR5 (Shared)".into();
    }
    match dev {
        0x6863 | 0x6864 | 0x6867 | 0x686C | 0x687F | 0x6860 | 0x6861 => "HBM2".into(),
        0x7360 | 0x73A0 | 0x73AB => "HBM2e".into(),
        0x67DF | 0x67EF | 0x67FF | 0x6FDF | 0x699F | 0x67C0 | 0x67E0 => "GDDR5".into(),
        _ => "GDDR6".into(),
    }
}

fn read_gpu_sysfs(index: u32, card: &str, dev: &Path) -> GpuInfo {
    let device = read_hex(dev.join("device")).unwrap_or(0);
    let bdf = std::fs::read_link(dev)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();

    let vram_total_mb = read_u64(&dev.join("mem_info_vram_total")).map(|b| (b / 1024 / 1024) as u32).unwrap_or(0);
    let vram_used_mb = read_u64(&dev.join("mem_info_vram_used")).map(|b| (b / 1024 / 1024) as u32).unwrap_or(0);

    let hwmon = amdgpu_hwmon(dev);
    let h = |n: &str| hwmon.as_ref().and_then(|d| read_trim(&d.join(n)));

    // Kernel docs: temp1 = edge, temp2/temp3 = hotspot/mem on dGPUs (millidegree C).
    let millic = |s: String| s.parse::<f32>().ok().map(|v| v / 1000.0);
    let temp_edge_c = h("temp1_input").and_then(millic);
    let temp_hotspot_c = h("temp2_input").and_then(millic);
    let temp_vram_c = h("temp3_input").and_then(millic);

    // hwmon freqN_input is Hz; fall back to the active pp_dpm_* level (Mhz).
    let gfx_clock_mhz = h("freq1_input")
        .and_then(|s| s.parse::<u64>().ok())
        .map(|hz| (hz / 1_000_000) as u32)
        .or_else(|| read_trim(&dev.join("pp_dpm_sclk")).and_then(|t| parse_pp_dpm_active(&t)));
    let mem_clock_mhz = h("freq2_input")
        .and_then(|s| s.parse::<u64>().ok())
        .map(|hz| (hz / 1_000_000) as u32)
        .or_else(|| read_trim(&dev.join("pp_dpm_mclk")).and_then(|t| parse_pp_dpm_active(&t)));

    let gfx_util_percent = read_trim(&dev.join("gpu_busy_percent"))
        .and_then(|s| s.parse::<u32>().ok())
        .map(|v| v.min(100));

    // power1_average (fallback power1_input) is microwatts, incl. CPU on APUs.
    let uw_to_w = |s: String| s.parse::<f64>().ok().map(|uw| (uw / 1e6) as f32);
    let power_w = h("power1_average").or_else(|| h("power1_input")).and_then(uw_to_w);
    let power_cap_w = h("power1_cap").and_then(uw_to_w);

    let pcie_width = read_trim(&dev.join("current_link_width"))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let pcie_speed_gt = read_trim(&dev.join("current_link_speed"))
        .and_then(|s| parse_link_speed_gts(&s))
        .unwrap_or(0);

    let market_name = read_trim(&dev.join("product_name"))
        .filter(|s| !s.eq_ignore_ascii_case("unknown"))
        .unwrap_or_else(|| format!("AMD GPU {}", card));
    let asic_serial = read_trim(&dev.join("unique_id"))
        .or_else(|| read_trim(&dev.join("serial_number")))
        .unwrap_or_default();

    GpuInfo {
        index,
        market_name,
        vendor_id: 0x1002,
        vendor_name: "AMD".into(),
        device_id: device as u64,
        subsystem_id: read_hex(dev.join("subsystem_device")).unwrap_or(0),
        rev_id: read_hex(dev.join("revision")).unwrap_or(0),
        asic_serial,
        num_cu: 0, // sysfs has no CU count; HIP merge may fill it
        gfx_version: crate::gpu::gfx_for_pci_dev(device),
        vram_type: linux_vram_type(device),
        vram_total_mb,
        vram_used_mb,
        vram_pinned_mb: 0, // no pinned-VRAM concept in sysfs (GTT is separate)
        vram_vendor: String::new(),
        bdf,
        pcie_width,
        pcie_speed_gt,
        driver_version: read_trim(Path::new("/sys/module/amdgpu/version")).unwrap_or_default(),
        vbios_version: read_trim(&dev.join("vbios_version")).unwrap_or_default(),
        temp_edge_c,
        temp_hotspot_c,
        temp_vram_c,
        gfx_clock_mhz,
        mem_clock_mhz,
        gfx_util_percent,
        power_w,
        power_cap_w,
        backend: "sysfs".into(),
        processes: Vec::new(), // filled by enrich_processes_linux
    }
}

impl Backend for LinuxSysfsBackend {
    fn discover(&self) -> anyhow::Result<Vec<GpuInfo>> {
        let cards = amd_cards();
        if cards.is_empty() {
            anyhow::bail!("no AMD drm cards in /sys/class/drm");
        }
        Ok(cards
            .iter()
            .enumerate()
            .map(|(i, (card, dev))| read_gpu_sysfs(i as u32, card, dev))
            .collect())
    }
}

/// Fill gaps sysfs can't provide (friendly name, gfx fallback, CU estimate)
/// from the HIP runtime when present. Index-merge: both enumerate PCI order.
pub fn enrich_with_hip(gpus: &mut Vec<GpuInfo>) {
    let hip = match super::hip::HipBackend.discover() {
        Ok(h) if !h.is_empty() => h,
        _ => return,
    };
    for (i, gpu) in gpus.iter_mut().enumerate() {
        let Some(h) = hip.get(i) else { continue };
        if gpu.market_name.starts_with("AMD GPU ") && !h.market_name.is_empty() {
            gpu.market_name.clone_from(&h.market_name);
        }
        if gpu.vram_total_mb == 0 && h.vram_total_mb > 0 {
            gpu.vram_total_mb = h.vram_total_mb;
        }
        if gpu.gfx_version == "unknown" && h.gfx_version != "unknown" {
            gpu.gfx_version.clone_from(&h.gfx_version);
        }
        if gpu.num_cu == 0 && h.num_cu > 0 {
            gpu.num_cu = h.num_cu;
        }
        if !gpu.backend.contains("hip") {
            gpu.backend = format!("{}+hip", gpu.backend);
        }
    }
}

// ---------------- per-process VRAM via /proc fdinfo ----------------

/// One DRM client from a `/proc/<pid>/fdinfo/<fd>` file. amdgpu emits
/// `drm-pdev: <bdf>`, `drm-client-id: <n>` plus `drm-resident-vram: <N> KiB`
/// (older kernels: `drm-memory-vram`). Values are per-client totals, so
/// callers must dedupe by (pid, client-id), not sum raw files.
#[derive(Default)]
struct FdClient {
    pdev: String,
    client: String,
    vram_kb: u64,
}

fn parse_kib(v: &str) -> Option<u64> {
    // "86867780 KiB" — kernel always reports KiB here
    v.split_whitespace().next()?.parse::<u64>().ok()
}

fn parse_fdinfo(text: &str, fallback_client: &str) -> FdClient {
    let mut c = FdClient { client: fallback_client.to_string(), ..Default::default() };
    let mut resident: Option<u64> = None;
    let mut legacy: Option<u64> = None;
    for line in text.lines() {
        let (k, v) = match line.split_once(':') {
            Some(x) => x,
            None => continue,
        };
        match k.trim() {
            "drm-pdev" => c.pdev = v.trim().to_lowercase(),
            "drm-client-id" => c.client = v.trim().to_string(),
            "drm-resident-vram" => resident = parse_kib(v),
            "drm-memory-vram" => legacy = parse_kib(v),
            _ => {}
        }
    }
    c.vram_kb = resident.or(legacy).unwrap_or(0);
    c
}

fn proc_name(pid: u32) -> String {
    if let Ok(t) = std::fs::read_link(format!("/proc/{}/exe", pid)) {
        if let Some(n) = t.file_name().and_then(|n| n.to_str()) {
            let n = n.trim().trim_end_matches(" (deleted)");
            if !n.is_empty() {
                return n.to_string();
            }
        }
    }
    if let Ok(c) = std::fs::read_to_string(format!("/proc/{}/comm", pid)) {
        let c = c.trim();
        if !c.is_empty() {
            return c.to_string();
        }
    }
    format!("pid{}", pid)
}

/// Best-effort per-process attribution (mirrors the Windows PDH path).
/// Needs no root for your own processes; run as root (or with CAP_SYS_PTRACE)
/// for the full system view — unreadable pids are skipped silently.
/// Caps total files scanned so `--serve` stays responsive on busy hosts.
pub fn enrich_processes_linux(gpus: &mut [GpuInfo]) {
    use std::collections::HashMap;
    if gpus.is_empty() {
        return;
    }
    let mut bdf_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, g) in gpus.iter().enumerate() {
        if !g.bdf.is_empty() {
            bdf_to_idx.insert(g.bdf.to_lowercase(), i);
        }
    }
    if bdf_to_idx.is_empty() {
        return;
    }

    // (pid, client-id) -> (gpu_idx, max_kb): max per client, since every fd of
    // one client repeats that client's totals.
    let mut clients: HashMap<(u32, String), (usize, u64)> = HashMap::new();
    let mut files_seen: u32 = 0;
    let pids: Vec<(u32, PathBuf)> = std::fs::read_dir("/proc")
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    e.file_name().to_string_lossy().parse::<u32>().ok().map(|pid| (pid, e.path()))
                })
                .collect()
        })
        .unwrap_or_default();

    'outer: for (pid, pdir) in pids {
        let entries = match std::fs::read_dir(pdir.join("fdinfo")) {
            Ok(rd) => rd,
            Err(_) => continue, // other users' processes / kernel threads
        };
        let mut per_pid = 0u32;
        for e in entries {
            if files_seen >= 8192 || per_pid >= 256 {
                if files_seen >= 8192 {
                    break 'outer;
                }
                break;
            }
            let e = match e {
                Ok(e) => e,
                Err(_) => continue,
            };
            let text = match std::fs::read_to_string(e.path()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            files_seen += 1;
            per_pid += 1;
            if !text.contains("drm-pdev") {
                continue; // fast path: not an amdgpu client
            }
            let c = parse_fdinfo(&text, &e.file_name().to_string_lossy());
            if c.pdev.is_empty() || c.vram_kb == 0 {
                continue;
            }
            let gi = match bdf_to_idx.get(&c.pdev) {
                Some(&i) => i,
                None => continue, // Intel/NVIDIA fd or unmapped card
            };
            clients
                .entry((pid, c.client))
                .and_modify(|v| {
                    if c.vram_kb > v.1 {
                        v.1 = c.vram_kb;
                    }
                })
                .or_insert((gi, c.vram_kb));
        }
    }

    // Sum distinct clients per (gpu, pid) — same overcount guard as Windows.
    let mut per_gpu_pid: HashMap<(usize, u32), u64> = HashMap::new();
    for ((pid, _), (gi, kb)) in clients {
        *per_gpu_pid.entry((gi, pid)).or_insert(0) += kb;
    }
    for ((gi, pid), kb) in per_gpu_pid {
        let mb = (kb / 1024) as u32;
        if mb == 0 {
            continue;
        }
        let gpu = &mut gpus[gi];
        if gpu.vram_total_mb > 0 && mb > gpu.vram_total_mb {
            continue;
        }
        let name = proc_name(pid);
        let proc_type = crate::gpu::classify_proc_type(&name);
        gpu.processes.push(ProcessInfo { pid, name, mem_used_mb: mb, proc_type });
    }
    for g in gpus.iter_mut() {
        g.processes.sort_by(|a, b| b.mem_used_mb.cmp(&a.mem_used_mb));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_name_filter() {
        assert!(is_drm_card_name("card0"));
        assert!(is_drm_card_name("card12"));
        assert!(!is_drm_card_name("card0-DP-1"));
        assert!(!is_drm_card_name("card0-HDMI-A-1"));
        assert!(!is_drm_card_name("renderD128"));
        assert!(!is_drm_card_name("controlD64"));
        assert!(!is_drm_card_name("card"));
    }

    #[test]
    fn pp_dpm_active_clock() {
        let t = "0: 615Mhz\n1: 800Mhz *\n2: 888Mhz\n";
        assert_eq!(parse_pp_dpm_active(t), Some(800));
        let deep = "S: 19Mhz *\n0: 615Mhz\n";
        assert_eq!(parse_pp_dpm_active(deep), Some(19));
        assert_eq!(parse_pp_dpm_active("0: 615Mhz\n1: 800Mhz\n"), None);
    }

    #[test]
    fn freq_parse() {
        assert_eq!(parse_freq_mhz("800Mhz"), Some(800));
        assert_eq!(parse_freq_mhz(" 1200Mhz "), Some(1200));
        assert_eq!(parse_freq_mhz("3.4Ghz"), Some(3400));
    }

    #[test]
    fn link_speed_parse() {
        assert_eq!(parse_link_speed_gts("16 GT/s PCIe"), Some(16));
        assert_eq!(parse_link_speed_gts("8 GT/s"), Some(8));
        assert_eq!(parse_link_speed_gts(""), None);
    }

    #[test]
    fn hex_parse() {
        assert_eq!(parse_hex_u32("0x1002"), Some(0x1002));
        assert_eq!(parse_hex_u32("744C"), Some(0x744C));
        assert_eq!(parse_hex_u32(""), None);
    }

    #[test]
    fn fdinfo_new_and_legacy_keys() {
        let new = "pos:\t0\nflags:\t02000002\ndrm-client-id:\t42\ndrm-pdev:\t0000:03:00.0\ndrm-resident-vram:\t86867780 KiB\n";
        let c = parse_fdinfo(new, "7");
        assert_eq!(c.pdev, "0000:03:00.0");
        assert_eq!(c.client, "42");
        assert_eq!(c.vram_kb, 86867780);

        // legacy key + missing client-id falls back to fd number
        let old = "drm-pdev:\t0000:03:00.0\ndrm-memory-vram:\t1408 KiB\n";
        let c = parse_fdinfo(old, "9");
        assert_eq!(c.client, "9");
        assert_eq!(c.vram_kb, 1408);

        // resident wins over legacy when both present
        let both = "drm-pdev:\t0000:03:00.0\ndrm-client-id:\t1\ndrm-memory-vram:\t100 KiB\ndrm-resident-vram:\t200 KiB\n";
        assert_eq!(parse_fdinfo(both, "3").vram_kb, 200);

        // non-amdgpu fd
        let other = "pos:\t0\nflags:\t0\n";
        let c = parse_fdinfo(other, "5");
        assert!(c.pdev.is_empty() && c.vram_kb == 0);
    }

    #[test]
    fn vram_type_spot_checks() {
        assert_eq!(linux_vram_type(0x744C), "GDDR6"); // 7900 XTX
        assert_eq!(linux_vram_type(0x7550), "GDDR6"); // 9070 XT
        assert_eq!(linux_vram_type(0x6860), "HBM2"); // Vega server
        assert_eq!(linux_vram_type(0x67DF), "GDDR5"); // Polaris
        assert_eq!(linux_vram_type(0x15BF), "DDR4/DDR5 (Shared)"); // Phoenix APU
    }
}
