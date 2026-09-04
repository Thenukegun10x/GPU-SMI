mod gpu;
mod backend;

use clap::Parser;
use backend::discover_all;

#[derive(Parser)]
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
    /// Bind address/hostname for --serve (default loopback). Use 0.0.0.0 for LAN scrape (requires token).
    #[arg(long, value_name="ADDR", default_value="127.0.0.1")] listen: String,
    /// Bearer token for --serve (visible in `ps`; prefer --token-file or GPU_SMI_TOKEN env).
    #[arg(long, value_name="TOKEN")] token: Option<String>,
    /// Read bearer token from file (or GPU_SMI_TOKEN_FILE env). Preferred: no `ps` exposure.
    #[arg(long, value_name="FILE")] token_file: Option<std::path::PathBuf>,
    /// Headless compact JSON for piping (one line per call, no pretty)
    #[arg(long)] compact: bool,
}

// Redact token from Debug so `{:?}` logs can't leak it.
impl std::fmt::Debug for Args {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Args")
            .field("json", &self.json)
            .field("verbose", &self.verbose)
            .field("show_ref", &self.show_ref)
            .field("watch", &self.watch)
            .field("serve", &self.serve)
            .field("listen", &self.listen)
            .field("token", &self.token.as_ref().map(|_| "[redacted]"))
            .field("token_file", &self.token_file)
            .field("compact", &self.compact)
            .finish()
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.show_ref {
        println!("Reference cloned to: amdsmi/include/amd_smi/amdsmi.h");
        println!("TheRock HIP runtime: C:\\TheRock\\build\\bin\\amdhip64_7.dll");
        return Ok(());
    }

    if let Some(port) = args.serve {
        let token = resolve_token(&args)?;
        return serve_headless(port, &args.listen, token);
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

fn serve_headless(port: u16, listen: &str, token: Option<String>) -> anyhow::Result<()> {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    // Fail closed: a non-loopback bind exposes process names/PIDs, so it
    // requires a bearer token. Local loopback stays zero-config.
    let remote = !is_loopback_bind(listen);
    if remote && token.is_none() {
        anyhow::bail!("refusing unauthenticated remote bind on {listen}:{port} — set --token/--token-file or GPU_SMI_TOKEN");
    }
    if remote {
        eprintln!("WARNING: listening on {listen}:{port} (non-loopback). Token auth required; put TLS/reverse-proxy in front if leaving the trusted LAN. Firewall it.");
    }
    let addr = format!("{}:{}", listen, port);
    let listener = TcpListener::bind(&addr).map_err(|e| anyhow::anyhow!("bind {} failed: {}", addr, e))?;
    println!("Headless API listening on http://{}/ (GET / -> JSON, GET /metrics -> Prometheus)", addr);
    if token.is_some() {
        println!("Auth: bearer token required for / and /metrics (/health open). Example: curl -H \"Authorization: Bearer <token>\" http://{}/metrics", addr);
    } else {
        println!("Auth: none (loopback only). Example: curl http://{}/  or  curl http://{}/metrics", addr, addr);
    }
    for stream in listener.incoming() {
        let mut stream = match stream { Ok(s)=>s, Err(_)=>continue };
        // Timeout so one slow/hung client can't wedge the single-threaded loop.
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        // Cap request size — headers only, we never need a body.
        let mut buf = [0u8; 8192];
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => continue,
            Ok(n) => n,
        };
        let req = String::from_utf8_lossy(&buf[..n]);
        let (method, path) = parse_request_line(&req);
        // Loopback-only mode: DNS-rebinding guard. Remote mode relies on the
        // bearer token instead (attacker site doesn't know it).
        if token.is_none() && !host_is_loopback(&req) {
            let body = r#"{"error":"forbidden host"}"#;
            let resp = format!("HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}", body.len(), body);
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            continue;
        }
        if method != "GET" {
            let body = r#"{"error":"method not allowed"}"#;
            let resp = format!("HTTP/1.1 405 Method Not Allowed\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}", body.len(), body);
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            continue;
        }
        // /health stays open (no sensitive data, useful for LB probes).
        // Everything else needs the token when one is configured.
        if let Some(expected) = token.as_ref() {
            if path != "/health" && !bearer_authorized(&req, expected) {
                let body = r#"{"error":"unauthorized"}"#;
                let resp = format!("HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nWWW-Authenticate: Bearer\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}", body.len(), body);
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                continue;
            }
        }
        let gpus = discover_all();
        let (status, body, ctype) = if path == "/metrics" {
            // Prometheus exposition format - memory safe, owned String per request
            let mut out = String::new();
            for g in &gpus {
                let id = g.index;
                let name = prom_escape(&g.market_name);
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
                    let pname = prom_escape(&p.name);
                    out.push_str(&format!("amd_gpu_process_mem_mb{{id=\"{}\",pid=\"{}\",name=\"{}\",type=\"{}\"}} {}\n", id, p.pid, pname, p.proc_type, p.mem_used_mb));
                }
            }
            ("HTTP/1.1 200 OK", out, "text/plain; version=0.0.4")
        } else if path == "/health" {
            ("HTTP/1.1 200 OK", r#"{"status":"ok"}"#.to_string(), "application/json")
        } else if path == "/" {
            (("HTTP/1.1 200 OK"), serde_json::to_string(&gpus).unwrap_or("[]".into()), "application/json")
        } else {
            ("HTTP/1.1 404 Not Found", r#"{"error":"not found"}"#.to_string(), "application/json")
        };
        // NOTE: intentionally no Access-Control-Allow-Origin. Adding CORS: *
        // would let any website you visit fetch 127.0.0.1 and exfiltrate your
        // process list. Same-origin curl/fetch from localhost still works.
        let resp = format!("{}\r\nContent-Type: {}\r\nContent-Length: {}\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}", status, ctype, body.len(), body);
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.flush();
    }
    Ok(())
}

/// Token precedence: --token-file > $GPU_SMI_TOKEN_FILE > --token > $GPU_SMI_TOKEN.
/// Flag/env values are trimmed; files may contain a trailing newline.
fn resolve_token(args: &Args) -> anyhow::Result<Option<String>> {
    let from_file = |p: &std::path::Path| -> anyhow::Result<String> {
        let raw = std::fs::read_to_string(p)
            .map_err(|e| anyhow::anyhow!("read token file {}: {}", p.display(), e))?;
        let t = raw.trim().to_string();
        if t.len() < 16 {
            anyhow::bail!("token in {} too short (need >=16 chars, use `openssl rand -hex 32`)", p.display());
        }
        warn_if_token_file_permissive(p);
        Ok(t)
    };
    if let Some(p) = &args.token_file {
        return Ok(Some(from_file(p)?));
    }
    if let Ok(p) = std::env::var("GPU_SMI_TOKEN_FILE") {
        if !p.trim().is_empty() {
            return Ok(Some(from_file(std::path::Path::new(p.trim()))?));
        }
    }
    if let Some(t) = &args.token {
        eprintln!("warning: --token is visible in process listings; prefer --token-file or GPU_SMI_TOKEN env");
        let t = t.trim().to_string();
        if t.len() < 16 {
            anyhow::bail!("--token too short (need >=16 chars, use `openssl rand -hex 32`)");
        }
        return Ok(Some(t));
    }
    if let Ok(t) = std::env::var("GPU_SMI_TOKEN") {
        let t = t.trim().to_string();
        if !t.is_empty() {
            if t.len() < 16 {
                anyhow::bail!("GPU_SMI_TOKEN too short (need >=16 chars)");
            }
            return Ok(Some(t));
        }
    }
    Ok(None)
}

#[cfg(unix)]
fn warn_if_token_file_permissive(p: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(md) = std::fs::metadata(p) {
        let mode = md.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            eprintln!("warning: token file {} has mode {:o} (group/other-readable); run `chmod 600 {}`", p.display(), mode, p.display());
        }
    }
}
#[cfg(not(unix))]
fn warn_if_token_file_permissive(_p: &std::path::Path) {}

/// True for loopback binds (127/8, localhost, ::1). Anything else — 0.0.0.0,
/// ::, LAN IP, hostname — counts as remote and requires a token.
fn is_loopback_bind(listen: &str) -> bool {
    let h = listen.trim().trim_matches(['[', ']']).to_lowercase();
    h == "127.0.0.1" || h.starts_with("127.") || h == "localhost" || h == "::1"
}

/// Constant-time string equality (no `subtle` dep, keeps single-exe small).
/// Length mismatch returns false; equal-length path runs in constant time.
fn ct_eq(a: &str, b: &str) -> bool {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    if ab.len() != bb.len() { return false; }
    let mut diff = 0u8;
    for i in 0..ab.len() { diff |= ab[i] ^ bb[i]; }
    diff == 0
}

/// Check `Authorization: Bearer <token>` (scheme case-insensitive).
fn bearer_authorized(req: &str, expected: &str) -> bool {
    for line in req.lines().skip(1) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() { break; }
        if line.len() < 14 { continue; }
        let (name, value) = match line.split_once(':') {
            Some(x) => x,
            None => continue,
        };
        if !name.eq_ignore_ascii_case("authorization") { continue; }
        let v = value.trim();
        let got = match v.split_once(' ') {
            Some((scheme, cred)) if scheme.eq_ignore_ascii_case("bearer") => cred.trim(),
            _ => continue,
        };
        return ct_eq(got, expected);
    }
    false
}

/// Parse `METHOD PATH HTTP/x` from the request line. Strips query string.
/// Returns ("","") on malformed input.
fn parse_request_line(req: &str) -> (&str, &str) {
    let line = req.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let raw_path = parts.next().unwrap_or("");
    if method.is_empty() || raw_path.is_empty() || !raw_path.starts_with('/') {
        return ("", "");
    }
    let path = raw_path.split('?').next().unwrap_or("/");
    (method, path)
}

/// Allow only loopback Host headers (mitigates DNS rebinding against the
/// unauthenticated local API). Missing Host (HTTP/1.0 curl) is allowed.
fn host_is_loopback(req: &str) -> bool {
    for line in req.lines().skip(1) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() { break; } // end of headers
        if let Some(v) = line.strip_prefix("Host:")
            .or_else(|| line.strip_prefix("host:"))
            .or_else(|| line.strip_prefix("HOST:"))
        {
            let h = v.trim().to_lowercase();
            // curl sends "127.0.0.1:8080", browsers may send "localhost:8080" or "[::1]:8080"
            return h.starts_with("127.0.0.1")
                || h.starts_with("localhost")
                || h.starts_with("[::1]")
                || h.starts_with("::1");
        }
    }
    true
}

/// Escape a Prometheus label value per exposition format: `\` -> `\\`,
/// `"` -> `\"`, newlines/CR -> space.
fn prom_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '"' => out.push_str(r#"\""#),
            '\n' | '\r' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
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
