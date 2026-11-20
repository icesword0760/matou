"""Read row numbers and thumb geometry from captured, composited window frames."""

import argparse
from itertools import groupby
import json
from pathlib import Path
import re
from statistics import median
import subprocess

from PIL import Image


ROW = re.compile(r"SCROLL\W*ROW\W*(\d{4})", re.IGNORECASE)


def runs(values):
    return [[value for _, value in group] for _, group in groupby(enumerate(values), lambda pair: pair[1] - pair[0])]


def read_rows(ocr, crop):
    strips = {}
    for hit in ocr["lines"]:
        match = ROW.search(hit["text"])
        if match:
            x, y, width, height = hit["box_px"]
            strips.setdefault(hit["strip_y_px"], []).append([int(match[1]), crop[0] + x, crop[1] + y, width, height])
    result = {}
    for strip, rows in strips.items():
        groups = []
        for row in rows:
            if groups and row[0] == groups[-1][-1][0] + 1:
                groups[-1].append(row)
            else:
                groups.append([row])
        group = max(groups, key=len, default=[])
        if len(group) >= 3:
            result["bottom" if strip else "top"] = group
    if "top" in result:
        rows = result["top"]
        result["first"] = rows[0][0]
        result["first_y_px"] = rows[0][2]
        result["line_px"] = median(current[2] - previous[2] for previous, current in zip(rows, rows[1:]))
    if "bottom" in result:
        result["last"] = result["bottom"][-1][0]
    return result


def thumb(image, thumb_x, y_min, y_max, scale):
    rgb = image.load()
    gutter = int(round(thumb_x))
    differences = {}
    for y in range(y_min, y_max):
        reference = rgb[gutter - round(7 * scale), y]
        if not 235 <= min(reference) <= 254:
            continue
        for x in range(gutter - round(2 * scale), gutter + round(3 * scale)):
            delta = reference[0] - rgb[x, y][0]
            if 1 <= delta <= 70:
                differences.setdefault(y, []).append((x, delta))
    candidates = [row for row in runs(sorted(differences)) if 32 * scale <= len(row) <= 120 * scale]
    if len(candidates) != 1:
        return {"status": "undetected" if not candidates else "ambiguous", "candidate_count": len(candidates)}
    rows = candidates[0]
    pixels = [pixel for y in rows for pixel in differences[y]]
    return {"status": "visible", "box_px": [min(p[0] for p in pixels), rows[0], max(p[0] for p in pixels) + 1, rows[-1] + 1],
            "peak_red_difference": max(p[1] for p in pixels)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("ocr", type=Path)
    parser.add_argument("thumb_x_pt", type=float)
    parser.add_argument("crop_pt", nargs=4, type=int)
    parser.add_argument("patterns", nargs="*", default=["*.png"])
    args = parser.parse_args()
    files = sorted({path for pattern in args.patterns for path in args.directory.glob(pattern) if "review" not in path.name})
    if not files:
        parser.error("No matching frame files")
    clips = args.directory / "row-ocr-clips"
    clips.mkdir(exist_ok=True)
    records = []
    for path in files:
        image = Image.open(path).convert("RGB")
        scale = image.width / 1200
        crop = tuple(round(point * scale) for point in args.crop_pt)
        image.crop(crop).save(clips / path.name)
        records.append({"file": path.name, "scale": scale, "crop_px": crop,
                        "thumb": thumb(image, args.thumb_x_pt * scale, crop[1], crop[3], scale)})
    raw = subprocess.check_output([str(args.ocr), "SCROLL", *(str(clips / path.name) for path in files)], text=True)
    for record, line in zip(records, raw.splitlines(), strict=True):
        data = json.loads(line)
        if data["file"] != record["file"]:
            raise SystemExit("OCR output order changed")
        record["rows"] = read_rows(data, record["crop_px"])
        print(json.dumps(record))


if __name__ == "__main__":
    main()
