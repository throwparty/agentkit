#!/usr/bin/env python3
import math
import os
import shutil
import statistics
import subprocess
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
HARNESS = os.path.join(HERE, "bwrap", "target", "release", "lbx-bwrap")
RUNS = 30
WARMUP = 3


def measure(command):
    for _ in range(WARMUP):
        subprocess.run(
            command,
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    samples = []
    for _ in range(RUNS):
        start = time.perf_counter()
        subprocess.run(
            command,
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        samples.append((time.perf_counter() - start) * 1000)
    samples.sort()
    return statistics.median(samples), samples[math.ceil(0.95 * len(samples)) - 1]


workspace = tempfile.mkdtemp(prefix="lbx-timing.")
host_home = tempfile.mkdtemp(prefix="lbx-timing-home.")
try:
    bare = measure(["true"])
    sandboxed = measure(
        [
            HARNESS,
            "--workspace",
            workspace,
            "--env",
            f"HOST_HOME={host_home}",
            "--",
            "true",
        ]
    )
finally:
    shutil.rmtree(workspace)
    shutil.rmtree(host_home)

print(f"runs:    {RUNS} after {WARMUP} warmup")
print(f"bare:    median {bare[0]:.1f}ms  p95 {bare[1]:.1f}ms")
print(f"sandbox: median {sandboxed[0]:.1f}ms  p95 {sandboxed[1]:.1f}ms")
