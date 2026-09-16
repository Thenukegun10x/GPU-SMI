"""Client for ``gpu-smi --serve`` (long-running HTTP API).

Beginners — poll one persistent process instead of spawning per tick::

    import gpu_smi
    with gpu_smi.serve(port=8080) as api:
        print(api.gpus()[0].summary())

Power users — point at an existing server (local or LAN) with a token::

    api = gpu_smi.ServeClient("http://ml-box:8080", token=open("token.txt").read())
    print(api.metrics())  # raw Prometheus text
"""
from __future__ import annotations

import contextlib
import os
import secrets
import stat
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from typing import Iterator

from ._core import BinaryNotFoundError, Gpu, GpuSmiError, QueryError, find_binary


class AuthError(QueryError):
    """The server rejected our credentials (HTTP 401)."""


class ServeClient:
    """Thin wrapper over the ``--serve`` HTTP API (stdlib only, no requests)."""

    def __init__(self, base_url: str = "http://127.0.0.1:8080",
                 token: str | None = None, timeout: float = 5.0) -> None:
        self.base_url = base_url.rstrip("/")
        self.token = token.strip() if token and token.strip() else None
        self.timeout = timeout

    def _get(self, path: str) -> bytes:
        req = urllib.request.Request(self.base_url + path, method="GET")
        if self.token:
            req.add_header("Authorization", f"Bearer {self.token}")
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                return resp.read()
        except urllib.error.HTTPError as e:
            if e.code == 401:
                raise AuthError("server rejected the bearer token (HTTP 401)") from e
            raise QueryError(f"server returned HTTP {e.code} for {path}") from e
        except OSError as e:
            raise QueryError(f"could not reach {self.base_url}{path}: {e}") from e

    def health(self) -> dict:
        """``{"status": "ok"}`` — open even when a token is configured."""
        import json
        return json.loads(self._get("/health").decode("utf-8"))

    def gpus(self) -> list[Gpu]:
        """Fresh snapshot as :class:`Gpu` objects (needs token when set)."""
        import json
        data = json.loads(self._get("/").decode("utf-8"))
        return [Gpu.from_dict(d) for d in data]

    def metrics(self) -> str:
        """Raw Prometheus exposition text from ``/metrics``."""
        return self._get("/metrics").decode("utf-8")

    def close(self) -> None:
        pass  # no persistent connections; here for API symmetry

    def __enter__(self) -> "ServeClient":
        return self

    def __exit__(self, *args: object) -> None:
        self.close()


@contextlib.contextmanager
def serve(port: int = 8080, listen: str = "127.0.0.1",
          token: str | bool | None = None,
          binary: str | None = None,
          timeout: float = 10.0) -> Iterator[ServeClient]:
    """Spawn ``gpu-smi --serve`` and yield a connected client; cleanup on exit.

    ``token=True`` auto-generates a 256-bit token and passes it via a
    mode-600 temp file (never via ``--token``, which leaks into ``ps``).
    """
    exe = find_binary(binary)
    tmp_token: str | None = None
    cmd = [exe, "--serve", str(port), "--listen", listen]
    if token is True:
        tmp_token = secrets.token_hex(32)
        token = tmp_token
    if isinstance(token, str) and token:
        fd, path = tempfile.mkstemp(prefix="gpu-smi-token-")
        try:
            if os.name == "posix":
                os.fchmod(fd, stat.S_IRUSR | stat.S_IWUSR)
            with os.fdopen(fd, "w") as f:
                f.write(token)
        except BaseException:
            os.close(fd)
            raise
        cmd += ["--token-file", path]
    else:
        path = ""

    try:
        proc = subprocess.Popen(
            cmd, stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE, text=True,
        )
    except OSError as e:
        if path:
            _silent_unlink(path)
        raise BinaryNotFoundError(f"could not execute {exe}: {e}") from e

    client = ServeClient(f"http://{listen}:{port}",
                         token=token if isinstance(token, str) else None)
    if listen in ("0.0.0.0", "::"):
        client.base_url = f"http://127.0.0.1:{port}"
    try:
        deadline = time.time() + timeout
        while True:
            if proc.poll() is not None:
                err = ((proc.stderr.read() if proc.stderr else "") or "").strip().splitlines()
                raise QueryError(
                    "gpu-smi --serve exited during startup: "
                    + (err[-1] if err else f"code {proc.returncode}"))
            try:
                client.health()
                break
            except GpuSmiError:
                if time.time() > deadline:
                    raise QueryError(f"gpu-smi --serve not ready after {timeout}s")
                time.sleep(0.1)
        yield client
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
        if proc.stderr:
            proc.stderr.close()
        if path:
            _silent_unlink(path)


def _silent_unlink(path: str) -> None:
    try:
        os.unlink(path)
    except OSError:
        pass
