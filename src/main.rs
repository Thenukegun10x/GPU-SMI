mod gpu;
mod backend;

use clap::Parser;
use backend::discover_all;

#[derive(Parser, Debug)]
#[command(name="gpu-smi", version, about="gpu-smi - portable single-exe GPU SMI for AMD GPUs (Windows+Linux). AMD-compatible, unofficial. Reference: amdsmi/include/amd_smi/amdsmi.h (MIT)")]
struct Args {
    /// Output JSON instead of table (headless API - pipe to your app)
    #[arg(long)] json: bool,
    /// Verbose (show backend used)
    #[arg(long)] verbose: bool,
    /// Show raw amdsmi.h reference path
    #[arg(long)] show_ref: bool,
    /// Realtime watch - refresh every N seconds (memory safe, no leaks). e.g. --watch 1
    #[arg(long, value_name="SECS", num_args=0..=1, default_missing_value="1")] watch: Option<u64>,
    /// Headless HTTP API - serve JSON on localhost:PORT (e.g. --serve 8080). Devs grab via curl/fetch
    #[arg(long, value_name="PORT", num_args=0..=1, default_missing_value="8080")] serve: Option<u16>,
    /// Headless compact JSON for piping (one line per call, no pretty)
    #[arg(long)] compact: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.show_ref {
        println!("Reference cloned to: amdsmi/include/amd_smi/amdsmi.h");
        println!("TheRock HIP runtime: C:\\TheRock\\build\\bin\\amdhip64_7.dll");
        return Ok(());
    }

    if let Some(port) = args.serve {
        return serve_headless(port);
    }

    if let Some(secs) = args.watch {
        let interval = std::time::Duration::from_secs(secs.max(1));
        loop {
            print!("\x1B[2J\x1B[1;1H");
            std::io::Write::flush(&mut std::io::stdout()).ok();
            if let Err(e) = render_once(&args) { eprintln!("discover error: {:#}", e); }
            std::io::Write::flush(&mut std::io::stdout()).ok();
            std::io::Write::flush(&mut std::io::stderr()).ok();
            std::thread::sleep(interval);
        }
    }

    render_once(&args)
}

fn serve_headless(port: u16) -> anyhow::Result<()> {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).map_err(|e| anyhow::anyhow!("bind {} failed: {}", addr, e))?;
    println!("Headless API listening on http://{}/ (GET / -> JSON, GET /metrics -> Prometheus)", addr);
    println!("Example: curl http://{}/  or  curl http://{}/metrics", addr, addr);
    for stream in listener.incoming() {
        let mut stream = match stream { Ok(s)=>s, Err(_)=>continue };
        let mut buf = [0u8; 2048];
        let _ = stream.read(&mut buf);
        let req = String::from_utf8_lossy(&buf);
        let is_metrics = req.contains("GET /metrics");
        let is_health = req.contains("GET /health");
        let gpus = discover_all();
        let (body, ctype) = if is_metrics {
            // Prometheus exposition format - memory safe, owned String per request
            let mut out = String::new();
            for g in &gpus {
                let id = g.index;
                let name = g.market_name.replace("\"","'");
                out.push_str(&format!("amd_gpu_vram_total_mb{{id=\"{}\",name=\"{}\"}} {}\n", id, name, g.vram_total_mb));
                out.push_str(&format!("amd_gpu_vram_used_mb{{id=\"{}\"}} {}\n", id, g.vram_used_mb));
                out.push_str(&format!("amd_gpu_vram_pinned_mb{{id=\"{}\"}} {}\n", id, g.vram_pinned_mb));
                if let Some(v)=g.temp_edge_c { out.push_str(&format!("amd_gpu_temp_edge_c{{id=\"{}\"}} {}\n", id, v)); }
                if let Some(v)=g.temp_hotspot_c { out.push_str(&format!("amd_gpu_temp_hotspot_c{{id=\"{}\"}} {}\n", id, v)); }
                if let Some(v)=g.gfx_clock_mhz { out.push_str(&format!("amd_gpu_gfx_clock_mhz{{id=\"{}\"}} {}\n", id, v)); }
                if let Some(v)=g.mem_clock_mhz { out.push_str(&format!("amd_gpu_mem_clock_mhz{{id=\"{}\"}} {}\n", id, v)); }
                if let Some(v)=g.gfx_util_percent { out.push_str(&format!("amd_gpu_util_percent{{id=\"{}\"}} {}\n", id, v)); }
                if let Some(v)=g.power_w { out.push_str(&format!("amd_gpu_power_watts{{id=\"{}\"}} {}\n", id, v)); }
                for p in &g.processes {
                    let pname = p.name.replace("\"", "'");
                    out.push_str(&format!("amd_gpu_process_mem_mb{{id=\"{}\",pid=\"{}\",name=\"{}\",type=\"{}\"}} {}\n", id, p.pid, pname, p.proc_type, p.mem_used_mb));
                }
            }
            (out, "text/plain; version=0.0.4")
        } else if is_health {
            (r#"{"status":"ok"}"#.to_string(), "application/json")
        } else {
            (serde_json::to_string(&gpus).unwrap_or("[]".into()), "application/json")
        };
        let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{}", ctype, body.len(), body);
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.flush();
    }
    Ok(())
}

fn render_once(args: &Args) -> anyhow::Result<()> {
    let gpus = discover_all();
    if gpus.is_empty() {
        eprintln!("No AMD GPU found. Tried: HIP (amdhip64) -> ADL (atiadlxx.dll) -> WMI (Win32_VideoController) on Windows; amdsmi+sysfs on Linux");
        eprintln!("Hint: check driver installed and C:\\TheRock\\build\\bin is in PATH");
        std::process::exit(2);
    }

    if args.json || args.compact {
        if args.compact {
            println!("{}", serde_json::to_string(&gpus)?);
        } else {
            println!("{}", serde_json::to_string_pretty(&gpus)?);
        }
        return Ok(());
    }

    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    println!("AMD SMI Portable | {} GPU(s) | {} | backend {} | Ctrl-C to exit", gpus.len(), ts, if args.watch.is_some() { format!("watch {}s", args.watch.unwrap_or(1)) } else { "one-shot".into() });
    // Header includes live sensors: VRAM (Used/Total), Pinned OS VRAM, Clocks, Util, Power
    println!("{:<3} {:<26} {:<8} {:<23} {:<9} {:<6} {:<8} {:<6} {:<8} {:<8}", "ID", "Name", "GFX", "VRAM (Used/Total)", "Pinned", "Temp", "GCLK", "Util", "Power", "Backend");
    println!("{}", "-".repeat(112));
    for g in &gpus {
        let vram = if g.vram_total_mb > 0 {
            if g.vram_used_mb > 0 {
                let pct = ((g.vram_used_mb as f64 / g.vram_total_mb as f64) * 100.0).round() as u32;
                format!("{} / {} MB ({}%)", g.vram_used_mb, g.vram_total_mb, pct)
            } else {
                format!("0 / {} MB", g.vram_total_mb)
            }
        } else {
            "-".into()
        };
        let pinned = if g.vram_pinned_mb > 0 {
            format!("{} MB", g.vram_pinned_mb)
        } else {
            "-".into()
        };
        let temp = g.temp_edge_c.map(|t| format!("{:.0}C", t)).unwrap_or("-".into());
        let gclk = g.gfx_clock_mhz.map(|v| format!("{}MHz", v)).unwrap_or("-".into());
        let util = g.gfx_util_percent.map(|v| format!("{}%", v)).unwrap_or("-".into());
        let power = g.power_w.map(|v| format!("{:.1}W", v)).unwrap_or("-".into());
        println!("{:<3} {:<26} {:<8} {:<23} {:<9} {:<6} {:<8} {:<6} {:<8} {:<8}",
            g.index, truncate(&g.market_name, 26), g.gfx_version, truncate(&vram, 23), pinned, temp, gclk, util, power, truncate(&g.backend, 8));
        if args.verbose {
            let hotspot = g.temp_hotspot_c.map(|t| format!("hotspot {:.0}C ", t)).unwrap_or_default();
            let vramt = g.temp_vram_c.map(|t| format!("vram {:.0}C ", t)).unwrap_or_default();
            let mclk = g.mem_clock_mhz.map(|v| format!("mclk {}MHz ", v)).unwrap_or_default();
            let free_mb = if g.vram_total_mb >= g.vram_used_mb { g.vram_total_mb - g.vram_used_mb } else { 0 };
            println!("    dev=0x{:04x} {} {} {} pcie=x{} {}GT/s driver {} | type: {} free: {}MB",
                g.device_id as u32, hotspot, vramt, mclk, g.pcie_width, g.pcie_speed_gt, truncate(&g.driver_version, 20), g.vram_type, free_mb);
        }
    }

    let mut all_procs: Vec<(u32, &crate::gpu::ProcessInfo)> = Vec::new();
    for g in &gpus {
        for p in &g.processes {
            all_procs.push((g.index, p));
        }
    }
    all_procs.sort_by(|a, b| b.1.mem_used_mb.cmp(&a.1.mem_used_mb));

    if !all_procs.is_empty() {
        println!("\nProcesses:");
        println!("{:<4} {:<8} {:<6} {:<42} {:>14}", "GPU", "PID", "Type", "Process Name", "Memory Usage");
        println!("{}", "-".repeat(76));
        let max_show = if args.verbose { all_procs.len() } else { 20 };
        for (gpu_idx, p) in all_procs.iter().take(max_show) {
            println!("{:<4} {:<8} {:<6} {:<42} {:>11} MB",
                gpu_idx, p.pid, p.proc_type, truncate(&p.name, 42), p.mem_used_mb);
        }
        if all_procs.len() > max_show {
            println!("  ... and {} more processes (--verbose to show all)", all_procs.len() - max_show);
        }
    }

    if !args.verbose { println!("\n(--watch 1 for realtime, --verbose for hotspot/vram temps, clocks & free VRAM, --json dump)"); }
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n { s.to_string() } else { format!("{}...", &s[..n-3]) }
}
