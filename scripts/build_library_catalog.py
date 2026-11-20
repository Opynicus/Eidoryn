"""Rebuild the bundled CC0 PS1 release metadata from Wikidata (network needed only by maintainers)."""
import argparse
from collections import defaultdict
import json
from pathlib import Path
import re
from urllib.parse import urlencode
from urllib.request import Request, urlopen

PLATFORM = "Q10677"
QUERY = """
SELECT DISTINCT ?game ?serial ?title ?region ?date ?precision ?alias ?platform ?datePlatform WHERE {
 ?game wdt:P400 wd:Q10677; p:P2606 ?id; rdfs:label ?title. FILTER(LANG(?title)="en")
 ?id ps:P2606 ?serial. OPTIONAL { ?id pq:P400 ?platform. }
 OPTIONAL { ?game skos:altLabel ?alias. FILTER(LANG(?alias)="en") }
 OPTIONAL { ?game p:P577 ?claim. ?claim pq:P291 ?region; psv:P577 ?time.
            ?time wikibase:timeValue ?date; wikibase:timePrecision ?precision.
            OPTIONAL { ?claim pq:P400 ?datePlatform } }
} ORDER BY ?serial
"""
JAPANESE = """
SELECT DISTINCT ?game ?title WHERE {
 ?game wdt:P400 wd:Q10677; wdt:P2606 ?serial; rdfs:label ?title. FILTER(LANG(?title)="ja")
}
"""
FIRST_PUBLICATION = """
SELECT DISTINCT ?game ?date ?precision WHERE {
 ?game wdt:P400 wd:Q10677; wdt:P2606 ?serial; p:P577 ?dateStatement.
 ?dateStatement psv:P577 ?time. ?time wikibase:timeValue ?date; wikibase:timePrecision ?precision.
 FILTER NOT EXISTS { ?dateStatement pq:P291 ?region }
 FILTER NOT EXISTS { ?dateStatement pq:P400 ?edition }
 FILTER NOT EXISTS { ?game wdt:P400 ?other. FILTER (?other != wd:Q10677) }
 FILTER NOT EXISTS { ?game p:P577 ?otherDate. FILTER (?otherDate != ?dateStatement) }
}
"""
# These PS1 product IDs have no per-ID P400 qualifier; see the snapshot notes.
REVIEWED = {"SCES-00344", "SCPS-10031", "SCUS-94900", "SCES-00001", "SLPS-00001", "SCUS-94300", "SLPS-01776"}
REGIONS = { "SCUS":"USA", "SLUS":"USA", "SCES":"Europe", "SLES":"Europe",
            "SLPS":"Japan", "SCPS":"Japan", "SIPS":"Japan", "SLPM":"Japan" }
PLACES = {"Q17": "Japan", "Q46":"Europe", "Q30":"USA", "Q49":"USA"}


def bindings(path, query):
    if path:
        response = Path(path).read_bytes()
    else:
        url = "https://query.wikidata.org/sparql?" + urlencode({"query": query, "format": "json"})
        with urlopen(Request(url, headers={"User-Agent": "Eidoryn-catalog/1 (github.com/Opynicus/Eidoryn)"}), timeout=90) as stream:
            response = stream.read()
    return [{k: (v["value"].rsplit("/", 1)[-1] if v["type"] == "uri" else v["value"])
             for k,v in row.items()} for row in json.loads(response)["results"]["bindings"]]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--records", help="cached JSON result of the release query")
    parser.add_argument("--japanese", help="cached JSON result of the Japanese-label query")
    parser.add_argument("--first-dates", help="cached JSON result of the work publication query")
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parents[1] / "src/library/metadata/ps1.tsv")
    args = parser.parse_args()
    local_names = {r["game"]: r["title"] for r in bindings(args.japanese, JAPANESE)}
    first_dates = {r["game"]:r["date"][:{"9":4, "10":7, "11":10}[r["precision"]]]
                   for r in bindings(args.first_dates, FIRST_PUBLICATION) if r["precision"] in ("9","10","11")}
    grouped = defaultdict(list)
    regions = defaultdict(set)
    for row in bindings(args.records, QUERY):
        serial = row["serial"]
        region = REGIONS.get(serial[:4])
        # Asian distribution uses SCPS-45xxx. This catalog has no verified
        # territory mapping for these editions, so do not call them Japanese.
        if serial.startswith("SCPS-45"):
            continue
        if not region or not re.fullmatch(r"[A-Z]{4}-[0-9]{5}", serial):
            continue
        if row.get("platform") != PLATFORM and not (row.get("platform") is None and serial in REVIEWED):
            continue
        grouped[(row["game"], region, serial)].append(row)
        regions[(row["game"], region)].add(serial)
    lines = []
    for (game, region, serial), group in sorted(grouped.items()):
        english = group[0]["title"]
        title = local_names.get(game, english) if region == "Japan" else english
        aliases = {english, *(r["alias"] for r in group if r.get("alias"))} - {title}
        dates = set()
        for row in group:
            if PLACES.get(row.get("region")) != region:
                continue
            if row.get("datePlatform") != PLATFORM and not (game == "Q719176" and "datePlatform" not in row):
                continue
            length = {"9":4, "10":7, "11":10}.get(row.get("precision"))
            if length:
                dates.add(row["date"][:length])
        # Wikidata places dates on the work. Several editions in one territory
        # cannot inherit the same date without additional edition-level evidence.
        if len(regions[(game, region)]) != 1:
            dates.clear()
        for date in sorted(dates or {""}):
            external_id = f"{game}/{serial}" + (f"/{date}" if len(dates) > 1 else "")
            parts = [external_id, serial, region, title, date, "|".join(sorted(aliases)), first_dates.get(game, "")]
            assert all("\n" not in value and "\t" not in value for value in parts)
            lines.append("\t".join(parts))
    args.output.write_text("\n".join(sorted(lines)) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
