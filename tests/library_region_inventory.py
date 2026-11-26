"""Count Libretro release and exact box-art records for every region in a DAT snapshot.

Usage: python3 tests/library_region_inventory.py DATABASE.dat github-tree.json [--matrix PS1|GBA]
The tree may be the Named_Boxarts directory or a complete recursive repository tree.
"""

import argparse
import csv
from hashlib import sha1
import json
import re
from collections import Counter, defaultdict
from pathlib import Path


def catalog(path):
    record = None
    for line in path.read_text(encoding="utf-8-sig").splitlines():
        line = line.strip()
        if line == "game (":
            if record is not None:
                raise ValueError("nested game entry")
            record = {}
        elif record is not None and line == ")":
            yield record
            record = None
        elif record is not None and (field := re.fullmatch(r'(name|region|serial) "(.*)"', line)):
            record[field[1]] = field[2]
    if record is not None:
        raise ValueError("incomplete game entry")


def boxarts(path):
    tree = json.loads(path.read_text(encoding="utf-8"))
    if tree["truncated"]:
        raise ValueError("incomplete artwork listing")
    prefix = "Named_Boxarts/" if any(
        node["path"].startswith("Named_Boxarts/") for node in tree["tree"]
    ) else ""
    files = {node["path"].removeprefix(prefix): node for node in tree["tree"]
             if node["type"] == "blob" and node["path"].startswith(prefix)
             and node["path"].endswith(".png")}
    if not files:
        raise ValueError("no Named_Boxarts PNGs")
    # Git stores a symlink as a blob whose entire content is the target filename.
    targets = {}
    for name in files:
        data = name.encode()
        header = f"blob {len(data)}\0".encode()
        targets[sha1(header + data).hexdigest()] = name
    images = {}
    for name, node in files.items():
        visited = set()
        while node["mode"] == "120000":
            target = targets.get(node["sha"])
            if target is None or target in visited:
                raise ValueError(f"unresolved or cyclic artwork link: {name}")
            visited.add(target)
            node = files[target]
        if node["mode"] != "100644":
            raise ValueError(f"unexpected artwork mode: {node}")
        images[name] = node["sha"]
    return images


def thumbnail_name(title):
    return re.sub(r'[&*/:`<>?\\|"]', "_", title) + ".png"


def release_market(record, platform):
    if record.get("region"):
        return record["region"]
    # Redump omits the region field for the UK and Scandinavia.
    if platform == "PS1":
        for market in ("UK", "Scandinavia"):
            if f"({market})" in record["name"]:
                return market
    return "(unspecified)"


def check_matrix(platform, records, images):
    fixture = Path(__file__).parent / "fixtures" / "regional-scrape.tsv"
    with fixture.open(encoding="utf-8") as rows:
        cases = [row for row in csv.reader(rows, delimiter="\t") if row[0] == platform]
    releases = {(r["name"], release_market(r, platform), r.get("serial", "-")) for r in records}
    markets = {market for _, market, _ in releases}
    difference = markets ^ {row[1] for row in cases}
    if difference:
        raise ValueError(f"region coverage differs from catalog: {sorted(difference)}")
    for _, market, serial, _, outcome, image, title in cases:
        if (title, market, serial) not in releases:
            raise ValueError(f"release differs from catalog: {market}, {serial}, {title}")
        expected = images.get(thumbnail_name(title), "-") if outcome == "matched" else "-"
        if expected != image:
            raise ValueError(f"regional artwork changed for {title}: expected {expected}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dat", type=Path)
    parser.add_argument("tree", type=Path)
    parser.add_argument("--matrix", choices=("PS1", "GBA"))
    args = parser.parse_args()
    images = boxarts(args.tree)
    counts = defaultdict(Counter)
    records = list(catalog(args.dat))
    if not records:
        raise ValueError("empty release catalog")
    if args.matrix:
        check_matrix(args.matrix, records, images)
    for record in records:
        row = counts[record.get("region", "(unspecified)")]
        row["records"] += 1
        row["with_serial"] += bool(record.get("serial"))
        row["with_art"] += thumbnail_name(record["name"]) in images
    print("region\trecords\twith_serial\twith_exact_boxart")
    for region, row in sorted(counts.items()):
        print(region, row["records"], row["with_serial"], row["with_art"], sep="\t")


if __name__ == "__main__":
    main()
