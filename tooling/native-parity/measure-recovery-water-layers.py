"""Report unmasked RGBA differences for the frozen and native static layers."""
import argparse
import json
from pathlib import Path

import numpy as np
from PIL import Image


def read(path):
    with Image.open(path) as png:
        return np.asarray(png.convert("RGBA"), dtype=np.int32)


def pad(image, shape):
    return np.pad(image, ((0, shape[0] - image.shape[0]),
                          (0, shape[1] - image.shape[1]), (0, 0)))


def premultiply(image):
    result = image.copy()
    result[:, :, :3] = (result[:, :, :3] * result[:, :, 3:] + 127) // 255
    return result


def metrics(delta, rect):
    absolute = np.abs(delta)
    h, w = rect
    y, x = np.where((absolute > 3).any(axis=2))
    edge = (x <= 0) | (x >= w - 1) | (y <= 0) | (y >= h - 1)
    return {
        "max_rgba": absolute.max(axis=(0, 1)).tolist(),
        "pixels_gt3": len(x),
        "pixels_gt3_within_1px_of_edge": int(edge.sum()),
        "pixels_gt3_outside_1px_of_edge": int((~edge).sum()),
    }


def compare(native, reference, output, names):
    rows = {}
    for name in names:
        a, b = read(native / f"{name}.png"), read(reference / f"{name}.png")
        shape = tuple(max(n, r) for n, r in zip(a.shape, b.shape))
        rect = tuple(min(n, r) for n, r in zip(a.shape[:2], b.shape[:2]))
        a, b = pad(a, shape), pad(b, shape)
        straight = a - b
        premult = premultiply(a) - premultiply(b)
        np.savez_compressed(output / f"{name}.npz",
                            straight_rgba=straight.astype(np.int16),
                            premultiplied_rgba=premult.astype(np.int16))
        rows[name] = {
            "png_size": list(shape[:2][::-1]),
            "layer_rect_size": list(rect[::-1]),
            "straight_rgba": metrics(straight, rect),
            "premultiplied_rgba": metrics(premult, rect),
        }
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("native", type=Path)
    parser.add_argument("reference", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--layers", nargs="+",
                        default=["base", "gradient", "stroke", "shadow", "edges", "combined"])
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    result = compare(args.native, args.reference, args.output, args.layers)
    (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
