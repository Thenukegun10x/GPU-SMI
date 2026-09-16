"""Unit tests — no GPU required. Binary calls are stubbed; HTTP is a live local stub server."""
import json
import os
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

import pytest

import gpu_smi
from gpu_smi import AuthError, BinaryNotFoundError, Gpu, NoGpuError, Process, QueryError

FULL = {
    "index": 1, "market_name": "AMD Radeon RX 9070 XT", "vendor_id": 4098,
    "vendor_name": "AMD", "device_id": 30032, "subsystem_id": 0, "rev_id": 0,
    "asic_serial": "", "num_cu": 64, "gfx_version": "gfx1201", "vram_type": "GDDR6",
    "vram_total_mb": 16304, "vram_used_mb": 2513, "vram_pinned_mb": 587,
    "vram_vendor": "", "bdf": "0000:03:00.0", "pcie_width": 16, "pcie_speed_gt": 16,
    "driver_version": "26.8.1", "vbios_version": "", "temp_edge_c": 39.0,
    "temp_hotspot_c": 44.0, "temp_vram_c": None, "gfx_clock_mhz": 750,
    "mem_clock_mhz": 2518, "gfx_util_percent": 22, "power_w": 8.2,
    "power_cap_w": 330.0, "backend": "hip+wmi+adl",
    "processes": [
        {"pid": 22232, "name": "Discord.exe", "mem_used_mb": 686, "proc_type": "G"},
        {"pid": 1234, "name": "python3.11", "mem_used_mb": 4096, "proc_type": "C"},
    ],
}
MINIMAL = {"index": 0, "market_name": "AMD GPU card0"}  # everything else defaults


def test_process_model():
    p = Process.from_dict(FULL["processes"][1])
    assert (p.pid, p.name, p.mem_mb, p.kind) == (1234, "python3.11", 4096, "C")
    assert p.is_compute and not Process.from_dict(FULL["processes"][0]).is_compute
    assert "1234" in str(p)


def test_gpu_full_mapping():
    g = Gpu.from_dict(FULL)
    assert g.name == "AMD Radeon RX 9070 XT"
    assert g.temp_c == 39.0 and g.hotspot_c == 44.0 and g.vram_c is None
    assert g.util_percent == 22 and g.power_w == 8.2 and g.num_cu == 64
    assert g.vram_free_mb == 16304 - 2513
    assert g.usage_ratio == pytest.approx(2513 / 16304)
    assert [p.name for p in g.processes] == ["Discord.exe", "python3.11"]
    assert g.raw["backend"] == "hip+wmi+adl"  # raw passthrough intact
    s = g.summary()
    assert "9070 XT" in s and "15%" in s and "39C" in s


def test_gpu_minimal_defaults():
    g = Gpu.from_dict(MINIMAL)
    assert g.temp_c is None and g.util_percent is None and g.processes == []
    assert g.usage_ratio is None and g.vram_free_mb == 0
    assert "AMD GPU card0" in g.summary()


def _completed(payload: str, code: int = 0):
    return subprocess.CompletedProcess(args=["gpu-smi"], returncode=code,
                                       stdout=payload, stderr="" if code == 0 else "boom")


@pytest.fixture(autouse=True)
def _fake_binary(monkeypatch):
    # _run_json resolves the binary before spawning; tests stub the spawn itself.
    monkeypatch.setattr(gpu_smi._core, "find_binary", lambda hint=None: "/fake/gpu-smi")


def test_query_end_to_end(monkeypatch, tmp_path):
    monkeypatch.setattr(subprocess, "run",
                        lambda *a, **k: _completed(json.dumps([FULL, MINIMAL])))
    monkeypatch.setenv("GPU_SMI_BIN", str(tmp_path / "gpu-smi"))  # value unused; run is stubbed
    got = gpu_smi.query()
    assert len(got) == 2 and got[0].name.endswith("9070 XT")
    assert gpu_smi.query_raw()[1]["market_name"] == "AMD GPU card0"
    assert gpu_smi.first().index == 1


def test_query_failures(monkeypatch):
    monkeypatch.setattr(subprocess, "run", lambda *a, **k: _completed("[]", code=3))
    with pytest.raises(QueryError):
        gpu_smi.query_raw()
    monkeypatch.setattr(subprocess, "run", lambda *a, **k: _completed("not json{{"))
    with pytest.raises(QueryError):
        gpu_smi.query_raw()
    monkeypatch.setattr(subprocess, "run", lambda *a, **k: _completed("{}"))
    with pytest.raises(QueryError):
        gpu_smi.query_raw()
    monkeypatch.setattr(subprocess, "run", lambda *a, **k: (_ for _ in ()).throw(
        subprocess.TimeoutExpired(cmd="x", timeout=1)))
    with pytest.raises(QueryError):
        gpu_smi.query_raw()
    monkeypatch.setattr(subprocess, "run", lambda *a, **k: _completed("[]"))
    with pytest.raises(NoGpuError):
        gpu_smi.first()
    assert gpu_smi.query() == []


def test_find_binary_precedence(monkeypatch, tmp_path):
    fake = tmp_path / "my-smi"
    fake.write_text("x")
    monkeypatch.setenv("GPU_SMI_BIN", str(fake))
    monkeypatch.setattr("shutil.which", lambda *a, **k: None)
    assert gpu_smi.find_binary() == str(fake)
    monkeypatch.delenv("GPU_SMI_BIN")
    bundled = gpu_smi.find_binary()  # falls back to the packaged binary
    assert bundled.endswith(os.path.join("bin", "gpu-smi.exe"))
    monkeypatch.setattr("os.path.isfile", lambda *a, **k: False)
    with pytest.raises(BinaryNotFoundError):
        gpu_smi.find_binary()


def test_watch_count(monkeypatch):
    calls = []
    monkeypatch.setattr(gpu_smi._core, "query",
                        lambda *a, **k: calls.append(1) or [Gpu.from_dict(MINIMAL)])
    snaps = list(gpu_smi.watch(interval=0, count=3))
    assert len(snaps) == 3 and all(s[0].name == "AMD GPU card0" for s in snaps)


# ---- ServeClient against a live stub server ----

TOKEN = "stub-token-0123456789abcdef"


class _Stub(BaseHTTPRequestHandler):
    def _send(self, code, body, ctype="application/json"):
        raw = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        if self.path == "/health":
            return self._send(200, '{"status":"ok"}')
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            return self._send(401, '{"error":"unauthorized"}')
        if self.path == "/metrics":
            return self._send(200, 'amd_gpu_vram_total_mb{id="1"} 16304\n',
                              "text/plain; version=0.0.4")
        if self.path == "/":
            return self._send(200, json.dumps([FULL]))
        return self._send(404, '{"error":"not found"}')

    def log_message(self, *a):
        pass


@pytest.fixture()
def stub_url():
    srv = HTTPServer(("127.0.0.1", 0), _Stub)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_port}"
    srv.shutdown()


def test_serve_client(stub_url):
    api = gpu_smi.ServeClient(stub_url, token=TOKEN)
    assert api.health() == {"status": "ok"}  # open endpoint
    gpus = api.gpus()
    assert gpus[0].vram_total_mb == 16304
    assert 'amd_gpu_vram_total_mb{id="1"} 16304' in api.metrics()


def test_serve_client_auth_failure(stub_url):
    with pytest.raises(AuthError):
        gpu_smi.ServeClient(stub_url, token="wrong").gpus()
    with pytest.raises(AuthError):
        gpu_smi.ServeClient(stub_url).metrics()  # no token at all
