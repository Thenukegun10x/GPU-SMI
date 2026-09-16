"""`python -m gpu_smi` — zero-code GPU report (summary, or --raw JSON)."""
import argparse
import json
import sys

import gpu_smi


def main() -> int:
    ap = argparse.ArgumentParser(prog="python -m gpu_smi", description="One-shot GPU report")
    ap.add_argument("--raw", action="store_true", help="print raw JSON instead of summaries")
    ap.add_argument("--bin", default=None, help="explicit gpu-smi binary path (or set GPU_SMI_BIN)")
    args = ap.parse_args()
    try:
        if args.raw:
            print(json.dumps(gpu_smi.query_raw(binary=args.bin), indent=2))
        else:
            gpus = gpu_smi.gpus(binary=args.bin)
            if not gpus:
                print("no GPUs reported")
            for gpu in gpus:
                print(gpu.summary())
                for p in gpu.processes:
                    print(f"    {p}")
    except gpu_smi.GpuSmiError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
