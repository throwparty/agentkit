#!/usr/bin/env python3
"""Measure apply+exec cost per prototype against a bare exec baseline.

Usage: timing.py <prototype-binary> <label> [runs]
"""

import math
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time


def measure(argv, runs):
    samples = []
    for _ in range(runs):
        start = time.perf_counter()
        result = subprocess.run(argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if result.returncode != 0:
            raise SystemExit(f"{argv[0]} exited {result.returncode}")
        samples.append((time.perf_counter() - start) * 1000.0)
    samples.sort()
    return samples


def percentile(samples, fraction):
    return samples[max(0, math.ceil(len(samples) * fraction) - 1)]


def main():
    if len(sys.argv) < 3:
        print(__doc__.strip(), file=sys.stderr)
        return 64

    binary = os.path.abspath(sys.argv[1])
    label = sys.argv[2]
    runs = int(sys.argv[3]) if len(sys.argv) > 3 else 30

    if not os.access(binary, os.X_OK):
        print(f"timing.py: {binary} is not executable", file=sys.stderr)
        return 64

    workspace = tempfile.mkdtemp(prefix=f"lbx-timing-{label}-")
    try:
        os.chdir(workspace)
        baseline = measure(["/usr/bin/true"], runs)
        samples = measure([binary, "--workspace", workspace, "--", "/usr/bin/true"], runs)

        print(f"{label}: bare exec median={statistics.median(baseline):.1f}ms")
        print(
            f"{label}: n={runs} min={samples[0]:.1f}ms "
            f"median={statistics.median(samples):.1f}ms "
            f"p95={percentile(samples, 0.95):.1f}ms max={samples[-1]:.1f}ms"
        )
        return 0
    finally:
        os.chdir("/")
        shutil.rmtree(workspace, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
