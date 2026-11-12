#!/usr/bin/env python3
"""Linux native renderer-switch accounting; no allocator replacement.

Build the renderer_memory_probe example first. Captures only its own process,
using the original demo and disconnected memory cards. See docs/RENDERER-MEMORY.md.
"""
import argparse
import collections
import json
import os
from pathlib import Path
import re
import subprocess
import time

from profile_ui_memory import binary_hash, niri_window, prepare_niri_window


PHASE = re.compile(r"^renderer-probe revision=(\d+) renderer=(\w+) running=true "
                   r"instructions=\d+ frames=\d+ error=None$", re.M)
EXPECTED = ["Serial", "Threaded", "Serial", "Vulkan", "Serial", "Vulkan"]


def kib_fields(text):
    return {key: int(value) for key, value in
            re.findall(r"^(\w+):\s*(\d+) kB$", text, re.M)}


def mappings(text):
    """Keep mapped size, resident pages, and proportionally shared pages distinct."""
    result = collections.defaultdict(collections.Counter)
    for block in re.split(r"(?=^[0-9a-f]+-[0-9a-f]+ )", text, flags=re.M):
        if not block:
            continue
        fields = block.splitlines()[0].split(maxsplit=5)
        name = fields[5] if len(fields) == 6 else "[anonymous]"
        numbers = kib_fields(block)
        result[name].update({key: numbers.get(key, 0) for key in
                             ("Size", "Rss", "Pss", "Private_Clean", "Private_Dirty")})
    return dict(result)


def drm_clients(folder):
    """Duplicate FDs for one DRM client must not count its memory twice."""
    clients = {}
    for path in folder.iterdir():
        try:
            text = path.read_text()
        except (FileNotFoundError, ProcessLookupError):
            continue
        fields = dict(line.split(":", 1) for line in text.splitlines()
                      if line.startswith("drm-"))
        fields = {key: value.strip() for key, value in fields.items()}
        if "drm-client-id" in fields:
            key = (fields.get("drm-pdev"), fields.get("drm-driver"), fields["drm-client-id"])
            clients[key] = fields
    # Driver residency may still include shared objects. Never add it to RSS.
    return list(clients.values())


def capture(args):
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ, XDG_CONFIG_HOME=str(output / "config"))
    for key in list(env):
        if key.startswith("HEAP_PROFILE") or key in ("LD_PRELOAD", "HEAPPROFILE", "HEAPPROFILESIGNAL"):
            del env[key]
    command = [str(binary), str(output / "trace.csv")]
    samples = []
    with (output / "run.log").open("w") as log:
        process = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        started = time.monotonic()
        try:
            if args.niri:
                prepare_niri_window(process)
            while process.poll() is None and time.monotonic() - started < 45:
                proc = Path(f"/proc/{process.pid}")
                try:
                    smaps = (proc / "smaps").read_text()
                    rollup = kib_fields((proc / "smaps_rollup").read_text())
                    status = kib_fields((proc / "status").read_text())
                    gpu = drm_clients(proc / "fdinfo")
                except (FileNotFoundError, ProcessLookupError):
                    if process.wait(timeout=5) != 0:
                        raise RuntimeError("Probe failed; inspect run.log")
                    break
                (output / f"{len(samples):02}-smaps").write_text(smaps)
                phases = list(PHASE.finditer((output / "run.log").read_text()))
                phase = phases[-1] if phases else None
                samples.append({
                    "seconds": time.monotonic() - started,
                    "revision": int(phase[1]) if phase else None,
                    "renderer": phase[2] if phase else None,
                    "process_KiB": rollup, "status_KiB": status,
                    "mappings_KiB": mappings(smaps), "gpu": gpu,
                    "window": niri_window(process.pid) if args.niri else None,
                })
                time.sleep(2)
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
    phases = [(int(m[1]), m[2]) for m in PHASE.finditer((output / "run.log").read_text())]
    valid = (process.returncode == 0 and phases == list(enumerate(EXPECTED))
             and {sample["revision"] for sample in samples} >= set(range(6)))
    result = {"binary_sha256": binary_hash(binary), "command": command,
              "returncode": process.returncode, "valid_sequence": valid,
              "driver_environment": {key: env[key] for key in
                                     ("VK_DRIVER_FILES", "VK_ICD_FILENAMES",
                                      "VK_LOADER_DRIVERS_SELECT", "VK_LOADER_DRIVERS_DISABLE")
                                     if key in env},
              "allocator": "native", "samples": samples}
    (output / "capture.json").write_text(json.dumps(result, indent=2) + "\n")
    if not valid:
        raise RuntimeError("Renderer sequence incomplete or failed; inspect run.log")
    for revision in range(6):
        sample = next(s for s in reversed(samples) if s["revision"] == revision)
        print(f"{revision} {sample['renderer']}: "
              f"RSS={sample['process_KiB']['Rss'] / 1024:.2f} MiB, "
              f"PSS={sample['process_KiB']['Pss'] / 1024:.2f} MiB")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--niri", action="store_true", help="Control only the probe's own window")
    capture(parser.parse_args())


if __name__ == "__main__":
    main()
