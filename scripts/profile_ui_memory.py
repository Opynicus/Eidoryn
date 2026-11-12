#!/usr/bin/env python3
"""Linux UI heap attribution with gperftools and llvm-symbolizer (no app hooks).

The profiler replaces malloc only in the measured child; do not interpret its RSS
as production RSS.
"""
import argparse
import bisect
import collections
import ctypes.util
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import time


ROW = re.compile(r"\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\]\s*@ (.*)")
FIELDS = ("live_objects", "live_bytes", "allocated_objects", "allocated_bytes")
# Order matters: shared renderer storage is not charged to the first icon/glyph
# that happened to grow an atlas or a scene buffer. Categories never overlap.
PATTERNS = (
    ("shared_rendering", r"gpui::scene::|gpui::platform::blade::blade_atlas|<gpui::window::Window>::paint_layer"),
    ("svg", r"usvg::|resvg::|gpui::svg_renderer|gpui::elements::svg|paint_svg"),
    ("fonts_text", r"cosmic_text::|fontdb::|swash::|rustybuzz::|gpui::text_system|gpui::platform::linux::text_system|gpui::elements::text|ttf_parser::"),
    # Recognize pixel ownership before the caller's layout/render stack. Moving
    # frame conversion into Render must not relabel its pixels as layout memory.
    ("image_buffers", r"image::|eidoryn::desktop::video::frame_image|gpui::platform::Image>::to_image_data"),
    ("emulation", r"eidoryn_ps1::|eidoryn_psx::systems::ps1::runner::snapshot::"),
    ("host_audio", r"eidoryn_host::audio::|cpal::|libpipewire|libspa-|\bpw_"),
    ("layout_elements", r"gpui::taffy|taffy::|gpui::elements::div|gpui::style::|gpui::arena::|gpui::element::|gpui::interactive::|eidoryn::desktop::(view|console|components|appearance|graphics|ps1_inspector)"),
)
# Actual rasterization under the atlas is still SVG/font work. Only atlas/scene
# storage itself belongs in shared_rendering.
RASTER = re.compile(r"gpui::svg_renderer|usvg::|resvg::|cosmic_text::|swash::|rasterize_glyph")
RULES = [(name, re.compile(pattern)) for name, pattern in PATTERNS]


def binary_hash(path):
    with open(path, "rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def category(stack):
    text = "\n".join(stack)
    for name, pattern in RULES:
        if name == "shared_rendering" and RASTER.search(text):
            continue
        if pattern.search(text):
            return name
    return "other"


def read_profile(path):
    table, maps = Path(path).read_text().split("MAPPED_LIBRARIES:\n", 1)
    entries = []
    for line in table.splitlines()[1:]:
        match = ROW.fullmatch(line)
        if match:
            # Profilers record return addresses. Attribute to the calling instruction.
            entries.append((tuple(map(int, match.groups()[:4])),
                            tuple(int(pc, 16) - 1 for pc in match[5].split())))
        elif line.strip():
            raise ValueError(f"Unrecognized heap row: {line[:80]}")
    header = ROW.fullmatch(table.splitlines()[0].removeprefix("heap profile:"))
    if header is None or tuple(sum(v[i] for v, _ in entries) for i in range(4)) != tuple(map(int, header.groups()[:4])):
        raise ValueError("Profile totals do not match allocation rows")
    return entries, maps


def elf_loads(path):
    with open(path, "rb") as file:
        header = file.read(64)
        if header[:5] != b"\x7fELF\x02":
            raise ValueError(f"Expected ELF64: {path}")
        order = "<" if header[5] == 1 else ">"
        offset = struct.unpack_from(order + "Q", header, 32)[0]
        size, count = struct.unpack_from(order + "HH", header, 54)
        file.seek(offset)
        headers = file.read(size * count)
    result = []
    for index in range(count):
        segment = struct.unpack_from(order + "IIQQQQQQ", headers, index * size)
        if segment[0] == 1:  # PT_LOAD: file offset and virtual address can differ.
            result.append((segment[2], segment[3]))
    return result


def symbolize(entries, maps, binary):
    segments, loads = [], {}
    executable = maps.splitlines()[0].split(maxsplit=5)[5]
    page = os.sysconf("SC_PAGE_SIZE")
    for line in maps.splitlines():
        fields = line.split(maxsplit=5)
        if len(fields) != 6 or "x" not in fields[1] or not fields[5].startswith("/"):
            continue
        path = str(binary) if fields[5] == executable else fields[5]
        if not Path(path).is_file():
            continue
        start, end = (int(part, 16) for part in fields[0].split("-"))
        offset = int(fields[2], 16)
        if path not in loads:
            loads[path] = elf_loads(path)
        for file_offset, virtual_address in loads[path]:
            if file_offset // page * page == offset:
                segments.append((start, end, start - virtual_address // page * page, path))
                break
    segments.sort()
    starts = [segment[0] for segment in segments]
    by_file = collections.defaultdict(list)
    for address in {pc for _, stack in entries for pc in stack}:
        index = bisect.bisect_right(starts, address) - 1
        if index >= 0:
            start, end, bias, path = segments[index]
            if start <= address < end:
                by_file[path].append((address, address - bias))
    symbols = {}
    for path, addresses in by_file.items():
        output = subprocess.run(
            ["llvm-symbolizer", "--output-style=JSON", "--demangle", "--obj=" + path],
            input="".join(hex(offset) + "\n" for _, offset in addresses),
            text=True, capture_output=True, check=True,
        )
        for (address, _), line in zip(addresses, output.stdout.splitlines(), strict=True):
            names = [item["FunctionName"] for item in json.loads(line)["Symbol"]]
            symbols[address] = "; ".join(filter(None, names)) or Path(path).name
    return symbols


def summarize(path, binary):
    entries, maps = read_profile(path)
    symbols = symbolize(entries, maps, binary)
    totals = collections.defaultdict(lambda: [0] * 4)
    lookup_errors = [0] * 4
    for values, addresses in entries:
        stack = [symbols.get(pc, "<unresolved>") for pc in addresses]
        bucket = totals[category(stack)]
        for index, value in enumerate(values):
            bucket[index] += value
        text = "\n".join(stack)
        if "::font_id" in text and "anyhow" in text:
            for index, value in enumerate(values):
                lookup_errors[index] += value
    return {
        "categories": {name: dict(zip(FIELDS, values)) for name, values in sorted(totals.items())},
        "font_lookup_errors": dict(zip(FIELDS, lookup_errors)),
    }


def smaps_summary(text):
    fonts = collections.Counter()
    for mapping in re.split(r"(?=^[0-9a-f]+-[0-9a-f]+ )", text, flags=re.MULTILINE):
        if not mapping or not re.search(r"\.(ttf|otf|ttc)(?: |$)", mapping.splitlines()[0], re.IGNORECASE):
            continue
        values = dict(re.findall(r"^(\w+):\s*(\d+) kB", mapping, re.MULTILINE))
        for name in ("Size", "Rss", "Pss"):
            fonts[name + "_KiB"] += int(values.get(name, 0))
    return dict(fonts)


def niri_window(pid):
    output = subprocess.run(["niri", "msg", "--json", "windows"],
                            text=True, capture_output=True, check=True)
    return next((window for window in json.loads(output.stdout) if window.get("pid") == pid), None)


def prepare_niri_window(process, fullscreen=False):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        window = niri_window(process.pid)
        if window:
            identifier = str(window["id"])
            actions = [] if fullscreen else [("move-window-to-floating", []),
                                              ("set-window-width", ["1280"]),
                                              ("set-window-height", ["900"]),
                                              ("center-window", [])]
            for action, values in [*actions, ("focus-window", [])]:
                subprocess.run(["niri", "msg", "action", action, "--id", identifier, *values],
                               check=True, capture_output=True)
            return
        if process.poll() is not None:
            break
        time.sleep(0.1)
    raise RuntimeError("Could not find the probe's own Niri window")


def capture(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    binary = args.binary.resolve(strict=True)
    env = dict(os.environ, XDG_CONFIG_HOME=str(output / "config"))
    # A native-allocator RSS run must not accidentally inherit profiling settings.
    for key in list(env):
        if key.startswith("HEAP_PROFILE") or key in ("LD_PRELOAD", "HEAPPROFILE", "HEAPPROFILESIGNAL"):
            del env[key]
    if not args.rss_only:
        library = ctypes.util.find_library("tcmalloc")
        if library is None:
            raise RuntimeError("gperftools libtcmalloc is required for heap capture")
        env.update(LD_PRELOAD=library, HEAPPROFILE=str(output / "heap"),
                   HEAP_PROFILE_ALLOCATION_INTERVAL="0", HEAP_PROFILE_INUSE_INTERVAL="0",
                   HEAP_PROFILE_TIME_INTERVAL=str(args.seconds))
    command = [str(binary), "--no-memory-cards", "--mute",
               "--theme", "midnight", "--renderer", "serial", "--crt", "off"]
    if not args.legacy_ui:
        command.extend(["--language", args.language])
    if args.view == "game":
        command.append("--game-view")
    elif args.view == "debugger":
        command.append("--debugger")
    elif args.view == "fullscreen":
        command.append("--fullscreen")
        if not args.legacy_ui:
            command.append("--clean-fullscreen")
    samples = []
    with (output / "run.log").open("w") as log:
        process = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        try:
            start = time.monotonic()
            if args.niri:
                prepare_niri_window(process, fullscreen=args.view == "fullscreen")
            for index in range(2):
                if args.rss_only:
                    time.sleep(args.seconds)
                else:
                    # Time-based dumping runs in the profiler's allocation hook.
                    # A signal may interrupt malloc while its heap lock is held.
                    # Skip the first dump, which may include startup activity.
                    profile = output / f"heap.{index + 2:04}.heap"
                    deadline, previous_size = time.monotonic() + args.seconds + 30, -1
                    while time.monotonic() < deadline:
                        time.sleep(0.1)
                        if process.poll() is not None:
                            raise RuntimeError(f"App exited ({process.returncode}); see {output}/run.log")
                        current_size = profile.stat().st_size if profile.exists() else 0
                        if current_size > 0 and current_size == previous_size:
                            try:
                                read_profile(profile)
                                break
                            except ValueError:
                                pass  # Maps/totals are not complete yet.
                        previous_size = current_size
                    else:
                        raise RuntimeError("Timed heap dump did not finish")
                    shutil.copyfile(profile, output / ("start.heap" if index == 0 else "end.heap"))
                if process.poll() is not None:
                    raise RuntimeError(f"App exited ({process.returncode}); see {output}/run.log")
                sample = {"seconds": time.monotonic() - start}
                if not args.rss_only:
                    sample["dump_unix_seconds"] = profile.stat().st_mtime_ns / 1e9
                if args.niri:
                    window = niri_window(process.pid)
                    if not window or not window["is_focused"]:
                        raise RuntimeError("Probe lost focus; discard this run and repeat without interacting")
                    sample["window"] = window
                for name in ("smaps", "smaps_rollup", "status"):
                    text = Path(f"/proc/{process.pid}/{name}").read_text()
                    (output / f"{index}-{name}").write_text(text)
                    if name == "smaps":
                        sample["font_mappings"] = smaps_summary(text)
                    elif name == "smaps_rollup":
                        sample["process_KiB"] = {key: int(value) for key, value in re.findall(r"^(\w+):\s*(\d+) kB", text, re.MULTILINE)}
                samples.append(sample)
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
    metadata = {"command": command, "binary_sha256": binary_hash(binary),
                "profiler": "none" if args.rss_only else "gperftools",
                "samples": samples}
    (output / "capture.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("capture", help="Open the built-in demo and sample twice")
    run.add_argument("--binary", type=Path, required=True)
    run.add_argument("--output", type=Path, required=True, help="New directory for this run")
    run.add_argument("--language", choices=("en", "zh-CN"), default="en")
    run.add_argument("--legacy-ui", action="store_true", help="Pre-redesign build: English only, no clean-fullscreen flag")
    run.add_argument("--view", choices=("console", "debugger", "game", "fullscreen"), default="console")
    run.add_argument("--seconds", type=int, choices=range(1, 31), default=5)
    run.add_argument("--rss-only", action="store_true")
    run.add_argument("--niri", action="store_true", help="Focus the probe; use 1280x900 floating for windowed views")
    report = commands.add_parser("report", help="Attribute a heap snapshot, optionally subtract an earlier one")
    report.add_argument("profile", type=Path)
    report.add_argument("--binary", type=Path, required=True, help="Exact executable used to capture this profile")
    report.add_argument("--base", type=Path)
    args = parser.parse_args()
    if args.command == "capture":
        if args.legacy_ui and args.language != "en":
            parser.error("--legacy-ui supports only --language en")
        capture(args)
    else:
        if shutil.which("llvm-symbolizer") is None:
            parser.error("llvm-symbolizer is required for report generation")
        digest = binary_hash(args.binary)
        for profile in (args.profile, args.base):
            if profile is None:
                continue
            metadata = profile.parent / "capture.json"
            if metadata.exists() and json.loads(metadata.read_text())["binary_sha256"] != digest:
                parser.error("Binary does not match capture.json; use the original executable")
        result = summarize(args.profile, args.binary.resolve(strict=True))
        if args.base:
            base = summarize(args.base, args.binary.resolve(strict=True))
            result["interval"] = {
                name: {field: values[field] - base["categories"].get(name, {}).get(field, 0)
                       for field in FIELDS}
                for name, values in result["categories"].items()
            }
            result["interval_font_lookup_errors"] = {
                field: result["font_lookup_errors"][field] - base["font_lookup_errors"][field]
                for field in FIELDS
            }
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
