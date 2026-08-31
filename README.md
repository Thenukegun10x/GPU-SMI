# gpu-smi — portable single-exe GPU SMI for AMD

**AMD-compatible, unofficial.** `Windows + Linux` portable `gpu-smi` that auto-discovers `HIP` / `ADL` / `WMI` / `sysfs` and exposes `VRAM`, `util`, `clocks`, `power`, `temps` in a `self-contained 900KB exe` — no installer, no `ROCm` SDK required to run.

> Reference implementation: `ROCm/amdsmi` `include/amd_smi/amdsmi.h` (MIT) cloned in `amdsmi/` for struct reference only (no link dependency). Uses `GPUOpen display-library` `ADL` headers (MIT) dynamically.

---

## Why

`amd-smi` is Linux-only and `ROCm` heavy. `gpu-smi` gives you one `exe` that runs on `Windows bare-metal` (where most AMD desktop users are) *and* `Linux`, with a headless `JSON`/`HTTP` API for devs.

## Features

* **Single exe** `913KB` (`--release lto+strip`) — `dumpbin /DEPENDENTS` only `kernel32`, `VCRUNTIME140`, `UCRT` (inbox on Win10/11). `atiadlxx.dll` is loaded from the AMD driver if present, else falls back to `WMI`. `amdhip64_7.dll` from `TheRock` (`C:\TheRock\build\bin`) is optional.
* **Auto-discovery** `HIP (TheRock) -> ADL (PMLog `ADL2_New_QueryPMLogData_Get`) -> WMI -> sysfs` with `ADL` `PMLog` temps/clocks/power/util live, `PCIe` width, `DDR5/DDR4/LPDDR/GDDR5/HBM2` `VRAM type` autodetected (not hardcoded `GDDR6`).
* **Headless API** for apps: `--json` / `--compact` pipe, `--serve 8080` `HTTP` `GET /` `GET /metrics` `GET /health`, `--watch N` realtime `ANSI` table (memory-safe, `create/destroy` per frame, no leak).
* **Memory safe** Rust ownership per request/frame, no shared `unsafe` state beyond `ADL`/`HIP` `dlopen` encapsulation.

## Quick start (Windows)

```ps
.\gpu-smi.exe                 # one-shot table
.\gpu-smi.exe --verbose       # BDF/CU/PCIe/hotspot
.\gpu-smi.exe --json          # pretty JSON (pipe to jq)
.\gpu-smi.exe --compact | python -c "import json,sys; print(json.load(sys.stdin)[1]['vram_total_mb'])"
.\gpu-smi.exe --watch 1       # realtime 1s refresh (Ctrl-C to exit)
.\gpu-smi.exe --serve 8080    # headless HTTP
curl http://127.0.0.1:8080/           # JSON
curl http://127.0.0.1:8080/metrics    # Prometheus
```

## Headless API for devs

### CLI pipe (simplest)

```py
import subprocess, json
gpus = json.loads(subprocess.check_output(["gpu-smi.exe","--compact"]))
print(gpus[0]["vram_total_mb"], gpus[1]["temp_edge_c"], gpus[1]["power_w"])
```

```js
// Node
const gpus = JSON.parse(require('child_process').execSync('gpu-smi.exe --compact').toString());
```

```powershell
# PowerShell
(gpu-smi.exe --compact | ConvertFrom-Json)[1].gfx_clock_mhz
```

### HTTP (long-running)

```ps
gpu-smi.exe --serve 8080
```
```py
import requests
gpus = requests.get("http://127.0.0.1:8080/").json()
# Prometheus scrape:
# http://127.0.0.1:8080/metrics  -> amd_gpu_vram_total_mb{id="1"} 16304
```

`GET /` -> `application/json` `Vec<GpuInfo>`, `GET /metrics` -> `text/plain; version=0.0.4` Prometheus, `GET /health` -> `{"status":"ok"}`, `CORS: *`.

### Schema (`GpuInfo` mirrors `amdsmi_asic_info_t` + `amdsmi_pcie_info_t`)

```json
{
  "index": 1,
  "market_name": "AMD Radeon RX 9070 XT",
  "device_id": 30032,
  "gfx_version": "gfx1201",
  "vram_type": "GDDR6",
  "vram_total_mb": 16304,
  "temp_edge_c": 39.0,
  "temp_hotspot_c": 44.0,
  "gfx_clock_mhz": 750,
  "gfx_util_percent": 22,
  "power_w": 8.2,
  "pcie_width": 16,
  "backend": "hip+wmi+adl"
}
```

## Build

```ps
cargo build --release
# -> target/release/gpu-smi.exe (or gpu-smi on Linux)
# Linux cross: cargo build --release --target x86_64-unknown-linux-gnu
# No bindgen/LLVM needed; ADL/HIP are dlopen at runtime.
```

## Project layout

```
gpu-smi/
  src/gpu.rs              # GpuInfo (mirrors amdsmi.h)
  src/backend/hip.rs      # HIP (amdhip64_7.dll) dlopen
  src/backend/windows.rs  # ADL PMLog + OverdriveN + WMI
  src/backend/linux.rs    # libamd_smi.so + sysfs
  amdsmi/                 # ROCm/amdsmi git clone (reference only, MIT)
  adl/                    # GPUOpen display-library (reference only, MIT)
```

## Trademark disclaimer

This project is not affiliated with, endorsed by, or sponsored by Advanced Micro Devices, Inc. `AMD`, `Radeon`, `RDNA`, `ROCm`, `Ryzen` and combinations thereof are trademarks of Advanced Micro Devices, Inc. in the United States and/or other jurisdictions. Other names are for informational purposes only. `gpu-smi` is a generic `GPU System Management Interface` shorthand, not an AMD mark, used descriptively for compatibility.

## License

`MIT` - see `LICENSE`.
