"""gpu-smi Python wrapper — friendly objects for beginners, raw power underneath.

Beginner::

    import gpu_smi
    for gpu in gpu_smi.gpus():          # no JSON, no subprocess knowledge needed
        print(gpu.summary())
        for p in gpu.processes:
            print(" ", p.pid, p.name, p.mem_mb, "MB")

Power user::

    import gpu_smi
    rows = gpu_smi.query_raw()          # raw list[dict], exactly as the binary emits
    with gpu_smi.serve(port=8080, token=True) as api:
        print(api.metrics())            # Prometheus text
    for snap in gpu_smi.watch(interval=0.5, count=10):
        ...
"""
from ._core import (
    BinaryNotFoundError,
    Gpu,
    GpuSmiError,
    NoGpuError,
    Process,
    QueryError,
    find_binary,
    first,
    gpus,
    query,
    query_raw,
    watch,
)
from ._serve import AuthError, ServeClient, serve

__all__ = [
    "AuthError",
    "BinaryNotFoundError",
    "Gpu",
    "GpuSmiError",
    "NoGpuError",
    "Process",
    "QueryError",
    "ServeClient",
    "find_binary",
    "first",
    "gpus",
    "query",
    "query_raw",
    "serve",
    "watch",
]

__version__ = "1.2.2"
