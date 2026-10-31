#!/usr/bin/env python3
"""GPU work of the desk while a hosted app animates and a window is dragged:
render-area pixels and pass count per second from the `gpu.pass` trace, and the
GPU time of every pass (MAKEPAD_VULKAN_PROFILE timestamps) per second.
    measure.py <checkout-with-release-build> <tag>
"""
import os, re, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rig_lib import Rig

root, tag = sys.argv[1], sys.argv[2]
g = Rig(root, tag, trace="gpu.pass", env={"MAKEPAD_VULKAN_PROFILE": "1"}).up()
try:
    g.launch("clock"); time.sleep(3)
    g.launch("notes"); time.sleep(14)  # past the launch animations
    def window(secs, drag=False):
        n0 = sum(1 for _ in open(g.log, errors="replace"))
        t0 = time.time()
        if drag:
            g.m("move", 640, 225); time.sleep(0.2); g.m("down", 640, 225)
            for i in range(int(secs * 10)):
                g.m("move", 600 + (i % 20) * 8, 300 + (i % 10) * 12); time.sleep(0.1)
            g.m("up", 600, 300)
        else:
            time.sleep(secs)
        dt = time.time() - t0
        lines = open(g.log, errors="replace").read().splitlines()[n0:]
        passes = px = 0
        gpu_ms = 0.0
        for l in lines:
            m = re.search(r"samples=(\d+) .*gpu_whole_pass_ms avg=([\d.]+)", l)
            if m: gpu_ms += int(m.group(1)) * float(m.group(2))
            m = re.search(r"offscreen done .* area=(\d+)", l)
            if m: passes += 1; px += int(m.group(1))
        return passes / dt, px / dt / 1e6, gpu_ms / dt
    for name, drag in (("idle", False), ("animating (clock)", False), ("dragging a window", True)):
        p, mpx, gms = window(8, drag)
        print(f"{name:22s} offscreen passes/s {p:8.1f}   Mpx/s {mpx:9.2f}   GPU ms/s {gms:8.1f}", flush=True)
finally:
    g.down()
