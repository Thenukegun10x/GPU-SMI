"""One-shot queries: find the gpu-smi binary, run it, return friendly objects.

Beginners: use :func:`gpus` / :func:`first` and read attributes off the
returned objects — no JSON involved::

    import gpu_smi
    for gpu in gpu_smi.gpus():
        print(gpu.name, gpu.vram_used_mb, "of", gpu.vram_total_mb, "MB")

Power users: :func:`query_raw` skips the object layer and hands you the
raw ``list[dict]`` exactly as the binary emitted it.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import time
from dataclasses import dataclass, field
from typing import Any, Iterator

# Never offer a .exe on POSIX (it can't execute there) — fall through to
# PATH / GPU_SMI_BIN with a clear error instead of a confusing exec failure.
_BINARY_NAMES = ("gpu-smi.exe", "gpu-smi") if os.name == "nt" else ("gpu-smi",)
_RELEASES_URL = "https://github.com/Thenukegun10x/GPU-SMI/releases"


class GpuSmiError(Exception):
    """Base class for everything this package raises."""


class BinaryNotFoundError(GpuSmiError):
    """The gpu-smi binary couldn't be located."""


class QueryError(GpuSmiError):
    """The binary failed, timed out, or its output couldn't be parsed."""


class NoGpuError(GpuSmiError):
    """Query succeeded but the binary reported zero GPUs."""


@dataclass
class Process:
    """One GPU-attached process. ``kind`` is ``"C"`` (compute) or ``"G"``."""

    pid: int
    name: str
    mem_mb: int
    kind: str = "G"

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "Process":
        return cls(
            pid=int(d.get("pid", 0)),
            name=str(d.get("name", "?")),
            mem_mb=int(d.get("mem_used_mb", 0)),
            kind=str(d.get("proc_type", "G")),
        )

    @property
    def is_compute(self) -> bool:
        return self.kind == "C"

    def __str__(self) -> str:
        return f"{self.name} (pid {self.pid}, {self.mem_mb} MB, {self.kind})"


@dataclass
class Gpu:
    """One GPU snapshot. Optionals are ``None`` when the backend lacks them."""

    index: int = 0
    name: str = "unknown"
    vendor_name: str = "AMD"
    device_id: int = 0
    gfx_version: str = "unknown"
    vram_type: str = "unknown"
    vram_total_mb: int = 0
    vram_used_mb: int = 0
    vram_pinned_mb: int = 0
    temp_c: float | None = None      # edge
    hotspot_c: float | None = None
    vram_c: float | None = None
    gfx_clock_mhz: int | None = None
    mem_clock_mhz: int | None = None
    util_percent: int | None = None  # gfx utilization
    power_w: float | None = None
    power_cap_w: float | None = None
    pcie_width: int = 0
    pcie_speed_gt: int = 0
    bdf: str = ""
    driver_version: str = ""
    vbios_version: str = ""
    backend: str = ""
    num_cu: int = 0
    processes: list[Process] = field(default_factory=list)
    raw: dict[str, Any] = field(default_factory=dict, repr=False)

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "Gpu":
        return cls(
            index=int(d.get("index", 0)),
            name=str(d.get("market_name", "unknown")),
            vendor_name=str(d.get("vendor_name", "AMD")),
            device_id=int(d.get("device_id", 0)),
            gfx_version=str(d.get("gfx_version", "unknown")),
            vram_type=str(d.get("vram_type", "unknown")),
            vram_total_mb=int(d.get("vram_total_mb", 0)),
            vram_used_mb=int(d.get("vram_used_mb", 0)),
            vram_pinned_mb=int(d.get("vram_pinned_mb", 0)),
            temp_c=_opt_float(d.get("temp_edge_c")),
            hotspot_c=_opt_float(d.get("temp_hotspot_c")),
            vram_c=_opt_float(d.get("temp_vram_c")),
            gfx_clock_mhz=_opt_int(d.get("gfx_clock_mhz")),
            mem_clock_mhz=_opt_int(d.get("mem_clock_mhz")),
            util_percent=_opt_int(d.get("gfx_util_percent")),
            power_w=_opt_float(d.get("power_w")),
            power_cap_w=_opt_float(d.get("power_cap_w")),
            pcie_width=int(d.get("pcie_width", 0) or 0),
            pcie_speed_gt=int(d.get("pcie_speed_gt", 0) or 0),
            bdf=str(d.get("bdf", "") or ""),
            driver_version=str(d.get("driver_version", "") or ""),
            vbios_version=str(d.get("vbios_version", "") or ""),
            backend=str(d.get("backend", "") or ""),
            num_cu=int(d.get("num_cu", 0) or 0),
            processes=[Process.from_dict(p) for p in d.get("processes", []) or []],
            raw=dict(d),
        )

    @property
    def vram_free_mb(self) -> int:
        return max(0, self.vram_total_mb - self.vram_used_mb)

    @property
    def usage_ratio(self) -> float | None:
        """VRAM used/total as 0..1, or ``None`` when total is unknown."""
        if self.vram_total_mb <= 0:
            return None
        return self.vram_used_mb / self.vram_total_mb

    def summary(self) -> str:
        parts = [f"[{self.index}] {self.name}"]
        if self.vram_total_mb:
            pct = f" ({self.usage_ratio * 100:.0f}%)" if self.usage_ratio is not None else ""
            parts.append(f"VRAM {self.vram_used_mb}/{self.vram_total_mb} MB{pct}")
        if self.temp_c is not None:
            parts.append(f"{self.temp_c:.0f}C")
        if self.util_percent is not None:
            parts.append(f"{self.util_percent}% util")
        if self.power_w is not None:
            parts.append(f"{self.power_w:.1f}W")
        if self.processes:
            top = self.processes[0]
            parts.append(f"top: {top.name} {top.mem_mb} MB")
        return " | ".join(parts)

    def __str__(self) -> str:
        return self.summary()


def _opt_float(v: Any) -> float | None:
    return None if v is None else float(v)


def _opt_int(v: Any) -> int | None:
    return None if v is None else int(v)


def find_binary(hint: str | None = None) -> str:
    """Locate the gpu-smi binary.

    Order: explicit ``hint`` > ``GPU_SMI_BIN`` env > ``PATH`` >
    ``gpu_smi/bin/`` next to this package (where wheels bundle it).
    Explicit configuration always beats implicit defaults.
    """
    candidates: list[str] = []
    if hint:
        candidates.append(hint)
    env = os.environ.get("GPU_SMI_BIN")
    if env:
        candidates.append(env)
    for c in candidates:
        if c and os.path.isfile(c):
            return c
    for name in _BINARY_NAMES:
        found = shutil.which(name)
        if found:
            return found
    here = os.path.dirname(os.path.abspath(__file__))
    for name in _BINARY_NAMES:
        bundled = os.path.join(here, "bin", name)
        if os.path.isfile(bundled):
            return bundled
    raise BinaryNotFoundError(
        "gpu-smi binary not found. Install it (see " + _RELEASES_URL + "), "
        "put it on PATH, or set GPU_SMI_BIN to its full path."
    )


def _run_json(args: list[str], binary: str | None, timeout: float) -> Any:
    exe = find_binary(binary)
    try:
        proc = subprocess.run(
            [exe, *args],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as e:
        raise QueryError(f"gpu-smi timed out after {timeout}s") from e
    except OSError as e:
        raise QueryError(f"could not execute {exe}: {e}") from e
    if proc.returncode != 0:
        err = (proc.stderr or "").strip().splitlines()
        raise QueryError(f"gpu-smi exited with code {proc.returncode}: {err[-1] if err else 'no output'}")
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError as e:
        raise QueryError(f"could not parse gpu-smi output: {e}") from e


def query_raw(binary: str | None = None, timeout: float = 10.0) -> list[dict[str, Any]]:
    """Raw ``list[dict]`` exactly as the binary emitted it (power users)."""
    data = _run_json(["--compact"], binary, timeout)
    if not isinstance(data, list):
        raise QueryError(f"unexpected gpu-smi output (wanted a list, got {type(data).__name__})")
    return data


def query(binary: str | None = None, timeout: float = 10.0) -> list[Gpu]:
    """One snapshot as a list of :class:`Gpu` (possibly empty)."""
    return [Gpu.from_dict(d) for d in query_raw(binary, timeout)]


def gpus(binary: str | None = None, timeout: float = 10.0) -> list[Gpu]:
    """Beginner alias for :func:`query`."""
    return query(binary, timeout)


def first(binary: str | None = None, timeout: float = 10.0) -> Gpu:
    """The first GPU — handy on single-GPU machines. Raises :class:`NoGpuError`."""
    all_gpus = query(binary, timeout)
    if not all_gpus:
        raise NoGpuError("gpu-smi reported zero GPUs")
    return all_gpus[0]


def watch(
    interval: float = 1.0,
    count: int | None = None,
    binary: str | None = None,
    timeout: float = 10.0,
) -> Iterator[list[Gpu]]:
    """Yield a fresh snapshot every ``interval`` seconds, forever or ``count`` times.

    For sub-second or multi-consumer polling, prefer :mod:`gpu_smi.serve`
    (one persistent process instead of a spawn per tick).
    """
    n = 0
    while count is None or n < count:
        yield query(binary, timeout)
        n += 1
        if count is None or n < count:
            time.sleep(interval)
