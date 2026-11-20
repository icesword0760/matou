"""Read exact RGB pixels from real window captures of terminal preedit."""

import argparse
from collections import Counter, deque
import json
from pathlib import Path

from PIL import Image, ImageChops


BLACK = (0, 0, 0)
WHITE = (255, 255, 255)


def read_overlay(path, seed):
    with Image.open(path) as source:
        screenshot = source.convert("RGBA")
    image = screenshot.convert("RGB")
    pixels = image.load()
    if pixels[seed] != BLACK:
        raise ValueError(f"{path}: seed {seed} is not pure black: {pixels[seed]}")

    # The empty part of the line box connects the black background around the glyphs.
    pending = deque([seed])
    background = {seed}
    while pending:
        x, y = pending.popleft()
        for neighbor in ((x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)):
            nx, ny = neighbor
            if (
                0 <= nx < image.width
                and 0 <= ny < image.height
                and neighbor not in background
                and pixels[neighbor] == BLACK
            ):
                background.add(neighbor)
                pending.append(neighbor)

    xs, ys = zip(*background)
    bounds = min(xs), min(ys), max(xs) + 1, max(ys) + 1
    if screenshot.getchannel("A").crop(bounds).getextrema() != (255, 255):
        raise ValueError(f"{path}: the measured box has nonopaque pixels")
    overlay = image.crop(bounds)
    if overlay.getpixel((0, 0)) != BLACK or overlay.getpixel(
        (overlay.width - 1, overlay.height - 1)
    ) != BLACK:
        raise ValueError(f"{path}: black area at {seed} has no rectangular corners")
    colors = Counter(overlay.getdata())
    return overlay, {
        "screenshot": str(path),
        "bbox_px_xyxy_exclusive": bounds,
        "size_px": overlay.size,
        "glyph_bounds_within_box_px_xyxy_exclusive": overlay.getbbox(),
        "black_pixels": colors[BLACK],
        "white_pixels": colors[WHITE],
        "other_pixels": overlay.width * overlay.height - colors[BLACK] - colors[WHITE],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("native", type=Path)
    parser.add_argument("--reference-seed", type=int, nargs=2, required=True, metavar=("X", "Y"))
    parser.add_argument("--native-seed", type=int, nargs=2, required=True, metavar=("X", "Y"))
    args = parser.parse_args()
    reference, ref_measurement = read_overlay(args.reference, tuple(args.reference_seed))
    native, native_measurement = read_overlay(args.native, tuple(args.native_seed))
    difference = None
    if reference.size == native.size:
        difference = sum(pixel != BLACK for pixel in ImageChops.difference(reference, native).getdata())
    print(json.dumps({
        "reference": ref_measurement,
        "native": native_measurement,
        "exact_different_pixels_at_matching_box_size": difference,
        "exact_box_pixels_match": difference == 0,
    }, indent=2))


if __name__ == "__main__":
    main()
