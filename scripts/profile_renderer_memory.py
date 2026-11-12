#!/usr/bin/env python3
"""Linux native renderer-switch accounting; no allocator replacement.

Build the renderer_memory_probe or backend_memory_probe example first. Captures
only its own process, with disconnected memory cards. The original demo is used
unless BIOS and disc paths are explicitly supplied. See docs/RENDERER-MEMORY.md.
"""
import argparse
import collections
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time

from profile_ui_memory import binary_hash, niri_window, prepare_niri_window


PHASE = re.compile(r"^renderer-probe revision=(\d+) renderer=(\w+) running=true "
                   r"instructions=\d+ frames=\d+ error=None$", re.M)
EXPECTED = ["Serial", "Threaded", "Serial", "Vulkan", "Serial", "Vulkan"]
BACKENDS = {
    "jit-first": [("Interpreter", "Serial"), ("Jit", "Serial"), ("Jit", "Vulkan"),
                  ("Interpreter", "Vulkan"), ("Interpreter", "Serial")],
    "vulkan-first": [("Interpreter", "Serial"), ("Interpreter", "Vulkan"), ("Jit", "Vulkan"),
                     ("Jit", "Serial"), ("Interpreter", "Serial")],
}


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


def backend_states(text):
    """Parse complete status lines; never substitute a requested mode for its acknowledgement."""
    states = []
    for line in text.splitlines(keepends=True):
        if not line.startswith("backend-probe ") or not line.endswith("\n"):
            continue
        fields, _, error = line.rstrip("\n").partition(" error=")
        values = dict(part.split("=", 1) for part in fields.split()[1:])
        states.append({
            "seconds": float(values["seconds"]), "cpu": values["cpu"],
            "renderer": values["renderer"], "revision": int(values["revision"]),
            "bios": values["bios"] == "true", "disc": values["disc"] == "true",
            "running": values["running"] == "true",
            "healthy": error == "None" and values["halted"] == "false",
            **{key: int(values[key]) for key in ("instructions", "frames", "blocks", "native")},
        })
    return states


def backend_transitions(states):
    transitions = []
    for state in states:
        modes = state["cpu"], state["renderer"]
        if not transitions or transitions[-1] != modes:
            transitions.append(modes)
    return transitions


def phase_identity(text, backend_sequence):
    if backend_sequence:
        states = backend_states(text)
        if not states:
            return None
        state = states[-1]
        return (len(backend_transitions(states)) - 1, state["cpu"], state["renderer"],
                state["running"], state["healthy"])
    phases = list(PHASE.finditer(text))
    return (int(phases[-1][1]), phases[-1][2]) if phases else None


def dgop_sample(pid):
    """Keep only our child PID's memory fields, not other processes or command lines."""
    result = subprocess.run(
        ["dgop", "processes", "--json", "--no-cpu", "--merge-children=false", "--limit", "0"],
        text=True, capture_output=True, check=True, timeout=10,
    )
    process = next((p for p in json.loads(result.stdout)["processes"] if p["pid"] == pid), None)
    if process is None:
        return None
    return {key: process.get(key) for key in ("memoryKB", "memoryCalculation", "rssKB", "pssKB")}


def capture(args):
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ, XDG_CONFIG_HOME=str(output / "config"))
    if args.backend_sequence:
        settings = output / "config/eidoryn/settings.json"
        settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({"language": args.language, "crt": args.crt, "theme": args.theme}))
    for key in list(env):
        if key.startswith("HEAP_PROFILE") or key in ("LD_PRELOAD", "HEAPPROFILE", "HEAPPROFILESIGNAL"):
            del env[key]
    command = [str(binary), str(output / "trace.csv")]
    if args.backend_sequence:
        command.append(args.backend_sequence)
        if args.bios:
            command.extend([str(args.bios.resolve(strict=True)), str(args.disc.resolve(strict=True))])
    samples = []
    with (output / "run.log").open("w") as log:
        process = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        started = time.monotonic()
        try:
            if args.niri:
                prepare_niri_window(process)
            while process.poll() is None and time.monotonic() - started < 45:
                phase_before = phase_identity((output / "run.log").read_text(), args.backend_sequence)
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
                monitor = dgop_sample(process.pid) if args.dgop else None
                log_text = (output / "run.log").read_text()
                phases = list(PHASE.finditer(log_text))
                phase = phases[-1] if phases else None
                states = backend_states(log_text)
                samples.append({
                    "seconds": time.monotonic() - started,
                    "revision": int(phase[1]) if phase else None,
                    "renderer": phase[2] if phase else None,
                    "process_KiB": rollup, "status_KiB": status,
                    "mappings_KiB": mappings(smaps), "gpu": gpu,
                    "window": niri_window(process.pid) if args.niri else None,
                    "backend_state": states[-1] if states else None,
                    "backend_phase": len(backend_transitions(states)) - 1,
                    "phase_stable": phase_before is not None and phase_before == phase_identity(log_text, args.backend_sequence),
                    "dgop": monitor,
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
    log_text = (output / "run.log").read_text()
    stable_samples = [sample for sample in samples if sample["phase_stable"]]
    if args.backend_sequence:
        states = backend_states(log_text)
        stable_samples = [sample for sample in stable_samples if sample["backend_state"]["running"]]
        valid = (process.returncode == 0 and all(state["healthy"] for state in states)
                 and backend_transitions(states) == BACKENDS[args.backend_sequence]
                 and {sample["backend_phase"] for sample in stable_samples} >= set(range(5)))
        phase_key, phase_count = "backend_phase", 5
    else:
        phases = [(int(m[1]), m[2]) for m in PHASE.finditer(log_text)]
        valid = (process.returncode == 0 and phases == list(enumerate(EXPECTED))
                 and {sample["revision"] for sample in stable_samples} >= set(range(6)))
        phase_key, phase_count = "revision", 6
    result = {"binary_sha256": binary_hash(binary), "command": command,
              "returncode": process.returncode, "valid_sequence": valid,
              "driver_environment": {key: env[key] for key in
                                     ("VK_DRIVER_FILES", "VK_ICD_FILENAMES",
                                      "VK_LOADER_DRIVERS_SELECT", "VK_LOADER_DRIVERS_DISABLE")
                                     if key in env},
              "backend_sequence": args.backend_sequence,
              "dgop_sha256": binary_hash(Path(shutil.which("dgop"))) if args.dgop else None,
              "allocator": "native", "samples": samples}
    (output / "capture.json").write_text(json.dumps(result, indent=2) + "\n")
    if not valid:
        raise RuntimeError("Renderer sequence incomplete or failed; inspect run.log")
    for revision in range(phase_count):
        sample = next(s for s in reversed(stable_samples) if s[phase_key] == revision)
        mode = (f"{sample['backend_state']['cpu']} + {sample['backend_state']['renderer']}"
                if args.backend_sequence else sample["renderer"])
        monitor = sample["dgop"]
        suffix = (f", DGOP={monitor['memoryKB'] / 1024:.2f} MiB [{monitor['memoryCalculation']}]"
                  if monitor else "")
        print(f"{revision} {mode}: "
              f"RSS={sample['process_KiB']['Rss'] / 1024:.2f} MiB, "
              f"PSS={sample['process_KiB']['Pss'] / 1024:.2f} MiB, "
              f"Pss_Dirty={sample['process_KiB']['Pss_Dirty'] / 1024:.2f} MiB{suffix}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--niri", action="store_true", help="Control only the probe's own window")
    parser.add_argument("--backend-sequence", choices=BACKENDS,
                        help="Use backend_memory_probe to isolate JIT and Vulkan transitions")
    parser.add_argument("--dgop", action="store_true", help="Also record DMS/dgop's display value and calculation method")
    parser.add_argument("--bios", type=Path)
    parser.add_argument("--disc", type=Path)
    parser.add_argument("--language", choices=("en", "zh-CN"), default="en")
    parser.add_argument("--crt", choices=("off", "soft", "classic"), default="off")
    parser.add_argument("--theme", default="midnight")
    args = parser.parse_args()
    if bool(args.bios) != bool(args.disc):
        parser.error("Provide both --bios and --disc, or neither")
    if not args.backend_sequence and (args.bios or args.language != "en" or args.crt != "off" or args.theme != "midnight"):
        parser.error("Media and appearance overrides require --backend-sequence")
    if args.dgop and shutil.which("dgop") is None:
        parser.error("--dgop requires the installed dgop executable")
    capture(args)


if __name__ == "__main__":
    main()
