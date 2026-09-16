# gpu-smi (Python wrapper)

Friendly Python access to [gpu-smi](https://github.com/Thenukegun10x/GPU-SMI) — stdlib only, no compiler, no dependencies.

You still need the `gpu-smi` binary itself (this package calls it for you). **Windows: it's bundled — `pip install` is all you need.** Linux: get it from [releases](https://github.com/Thenukegun10x/GPU-SMI/releases) or `cargo build --release`, then put it on `PATH` or set `GPU_SMI_BIN` (a Linux binary will be bundled in a later release the same way).

```sh
pip install gpu-smi
# from source: pip install ./python
```

## Beginner (no JSON needed)

```python
import gpu_smi

for gpu in gpu_smi.gpus():
    print(gpu.summary())
    # [1] AMD Radeon RX 9070 XT | VRAM 2513/16304 MB (15%) | 39C | 22% util | 8.2W

gpu = gpu_smi.first()          # single-GPU shortcut (raises NoGpuError if none)
print(gpu.name, gpu.temp_c, gpu.util_percent)
print(gpu.vram_free_mb, gpu.usage_ratio)
for p in gpu.processes:        # pid, name, mem_mb, kind ("C"/"G"), is_compute
    print(p.pid, p.name, p.mem_mb)
```

`python -m gpu_smi` prints the same report with zero code (`--raw` for JSON).

## Power user

```python
import gpu_smi

rows = gpu_smi.query_raw()                          # raw list[dict], untouched
for snap in gpu_smi.watch(interval=0.5, count=10):  # polling generator
    ...

# Long-running HTTP API (one persistent process, Prometheus included):
with gpu_smi.serve(port=8080, token=True) as api:   # token=True -> auto 256-bit token via mode-600 tempfile
    print(api.gpus()[0].summary())
    print(api.metrics())                            # raw Prometheus exposition text

api = gpu_smi.ServeClient("http://ml-box:8080", token=open("token.txt").read().strip())
api.health()  # {"status": "ok"} — open even with auth on
```

Binary lookup order: explicit arg > `GPU_SMI_BIN` env > `PATH` > bundled `gpu_smi/bin/`.
Errors: `BinaryNotFoundError`, `QueryError`, `AuthError` (bad bearer token), `NoGpuError` — all subclass `GpuSmiError`.

## License

MIT, same as gpu-smi.
