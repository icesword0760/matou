"""Read exact pixels in unshadowed, same-scale PNG captures of the settings window."""

import argparse
import io
import json
from collections import Counter
from pathlib import Path

from PIL import Image, ImageCms, ImageChops


def load(path, srgb=False):
    with Image.open(path) as image:
        if srgb and (profile := image.info.get("icc_profile")):
            return ImageCms.profileToProfile(
                image.convert("RGB"), io.BytesIO(profile), ImageCms.createProfile("sRGB")
            )
        return image.convert("RGB")


def extent(line, origin, color):
    indices = [index + origin for index, pixel in enumerate(line) if pixel == color]
    return [min(indices), max(indices) + 1] if indices else None


def sample(path, scale, dialog):
    raw = load(path)
    w, h = raw.size
    points = round(w / scale), round(h / scale)
    srgb = load(path, srgb=True)
    def px(x, y):
        return round(x * scale), round(y * scale)

    if dialog:
        center = (points[0] + 260) / 2
        # The straight white surface, sampled away from corners and controls.
        x, _ = px(center + 215, 0)
        white = (255, 255, 255)
        vertical = extent([raw.getpixel((x, i)) for i in range(h)], 0, white)
        row = vertical[0] + round(22 * scale) if vertical else None
        stage_start = px(260, 0)[0]
        return {
            "png_px": raw.size,
            "dialog_scan_row_px": row,
            "dialog_white_x_at_scan_row_px": extent(
                [raw.getpixel((i, row)) for i in range(stage_start, w)], stage_start, white
            ) if row is not None else None,
            "dialog_white_y_at_center_plus_215_px": vertical,
        }

    x, y = px(points[0] - 10, 10)
    canvas = raw.getpixel((x, y))
    x_start = px(275, 0)[0]
    background_row = [raw.getpixel((i, y)) for i in range(x_start, w)]
    first_canvas = w - 1
    while first_canvas > x_start and raw.getpixel((first_canvas - 1, y)) == canvas:
        first_canvas -= 1

    result = {
        "png_px": raw.size,
        "canvas_raw_rgb": canvas,
        "canvas_srgb": srgb.getpixel((x, y)),
        "stage_body_start_px_at_y10": first_canvas,
        "nav_divider_and_body_at_y10_raw_rgb": background_row[
            first_canvas - x_start - 2 : first_canvas - x_start + 2
        ],
    }
    if points == (1200, 780):
        # These bands deliberately avoid the label glyphs and rounded corners.
        y = px(0, 40)[1]
        x0, x1 = px(1130, 0)[0], px(1179, 0)[0]
        result["close_x_at_y40_px"] = extent(
            [raw.getpixel((i, y)) != canvas for i in range(x0, x1)], x0, True
        )
        left, right = px(465, 0)[0], px(1172, 0)[0]
        colors = Counter(
            raw.getpixel((i, j))
            for i in range(left, right)
            for j in range(px(0, 136)[1], px(0, 209)[1])
            if raw.getpixel((i, j)) != canvas
        )
        if colors:
            result["first_card_candidate_raw_rgb"] = colors.most_common(1)[0][0]
            card = result["first_card_candidate_raw_rgb"]
            x, y = px(850, 185)
            result["card_fill_x_at_y185_px"] = extent(
                [raw.getpixel((i, y)) for i in range(left, right)], left, card
            )
            start, end = px(0, 121)[1], px(0, 230)[1]
            fill_y = extent(
                [raw.getpixel((x, j)) for j in range(start, end)], start, card
            )
            result["card_fill_y_at_x850_px"] = fill_y
            if fill_y is not None:
                top, bottom = fill_y
                result["card_edge_samples_at_x850"] = {
                    "above_fill": [[j, raw.getpixel((x, j))] for j in range(top - 2, top)],
                    "below_fill": [[j, raw.getpixel((x, j))] for j in range(bottom, bottom + 2)],
                }
    return result


def compare(reference, native, scale, dialog):
    paths = {"reference": reference, "native": native}
    measurements = {name: sample(path, scale, dialog) for name, path in paths.items()}
    a, b = (load(path) for path in paths.values())
    if a.size == b.size and not dialog:
        # Crop to the provider region shared by the two pages. The lower
        # model-assignment and context sections are outside this delivery.
        box = tuple(round(value * scale) for value in (260, 0, 1200, 210))
        output = ImageChops.difference(a.crop(box), b.crop(box))
        measurements["raw_pixel_comparison"] = {
            "window_crop_px": box,
            "pixels_different_at_zero_tolerance": sum(
                pixel != (0, 0, 0) for pixel in output.getdata()
            ),
            "pixels_total": output.width * output.height,
        }
    return measurements


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("native", type=Path)
    parser.add_argument("--scale", type=float, required=True)
    parser.add_argument("--dialog", action="store_true")
    args = parser.parse_args()
    print(json.dumps(compare(args.reference, args.native, args.scale, args.dialog), indent=2))


if __name__ == "__main__":
    main()
