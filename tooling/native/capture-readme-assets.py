#!/usr/bin/env python3
"""Record the native macOS README demos against an offline, disposable profile.

Run with Python 3 and ffmpeg from a logged-in macOS session on a Retina display.
The calling terminal needs macOS Screen Recording permission. The native Cargo
dependencies and libghostty-vt must already be prepared in this checkout.
"""
import argparse
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
TARGET = ROOT / "apps/native/target"
PROBE = TARGET / "debug/examples/workspace_wiring_probe"

# Raw window captures are listed in playback order with their holds in seconds.
GIFS = {
    "workspace-demo": ("workspace", [2.0, 1.7, 1.5, 2.2, 1.7, 1.7, 2.2, 1.6, 1.8], [2, 4, 6, 9]),
    "provider-routing-demo": ("provider", [2.2, 2.5, 1.8, 2.2], [1, 2, 3, 4]),
    "account-switch-demo": ("account", [1.8, 2.3, 2.2], [1, 2, 3]),
    "dispatch-foreman-demo": ("dispatch", [2.5, 1.5, 2.5], [1, 2, 3]),
    "context-optimizer-demo": ("context", [2.4, 2.0, 2.2, 2.3], [1, 2, 3, 4]),
}

STILLS = {
    "workspace-demo": "workspace-01",
    "session-dag-demo": "workspace-07",
    "agent-hud-notifications-demo": "workspace-04",
    "workspace-board-demo": "workspace-08",
    "history-seam-demo": "history",
}


def run(*args, **options):
    options.setdefault("cwd", ROOT)
    subprocess.run(args, check=True, **options)


def ffmpeg(*args):
    run("ffmpeg", "-hide_banner", "-loglevel", "error", "-y", *args)


def record(locale, scratch):
    frames = scratch / "frames"
    frames.mkdir()
    home = scratch / "home"
    home.mkdir()
    temporary = scratch / "tmp"
    temporary.mkdir()
    # In particular, inherit no agent credentials, provider variables or app paths.
    env = {
        "HOME": str(home),
        "CFFIXED_USER_HOME": str(home),
        "TMPDIR": str(temporary),
        "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
        "LANG": "en_US.UTF-8",
        "TZ": "UTC",
        "MATOU_README_CAPTURE": frames.name,
        "MATOU_README_LOCALE": locale,
    }
    run(str(PROBE), cwd=scratch, env=env, timeout=90)
    return frames


def encode(frames, output):
    output.mkdir()
    for name, (prefix, holds, _) in GIFS.items():
        files = []
        for index in range(1, len(holds) + 1):
            source = frames / f"{prefix}-{index:02}.png"
            frame = frames / f"{prefix}-scaled-{index:02}.png"
            ffmpeg(
                "-i", str(source), "-vf",
                "scale=1200:738:force_original_aspect_ratio=decrease:flags=lanczos,"
                "pad=1200:738:(ow-iw)/2:(oh-ih)/2:color=0xf7f8fa,setsar=1,format=rgb24",
                "-frames:v", "1", str(frame),
            )
            files.append(frame)
        manifest = frames / f"{prefix}.ffconcat"
        # Relative names avoid quoting user paths in the concat file.
        lines = ["ffconcat version 1.0"]
        for file, hold in zip(files, holds):
            lines.extend([f"file {file.name}", "option framerate 100", f"duration {hold:.2f}"])
        manifest.write_text("\n".join(lines) + "\n")
        ffmpeg(
            "-f", "concat", "-safe", "0", "-i", str(manifest), "-filter_complex_threads", "1",
            "-filter_complex",
            "split[a][b];[a]palettegen=stats_mode=diff[p];"
            "[b][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle",
            "-fps_mode", "vfr", "-gifflags", "+offsetting+transdiff",
            "-loop", "0", "-final_delay", str(round(holds[-1] * 100)), str(output / f"{name}.gif"),
        )
    for name, scene in STILLS.items():
        ffmpeg(
            "-i", str(frames / f"{scene}.png"), "-vf", "scale=1600:-1:flags=lanczos",
            "-frames:v", "1", "-compression_level", "9", str(output / f"{name}.png"),
        )


def extract(output, preview):
    preview.mkdir(parents=True, exist_ok=True)
    for name, (_, _, indices) in GIFS.items():
        for index in indices:
            frame = preview / f"{name}-{index:02}.png"
            ffmpeg(
                "-i", str(output / f"{name}.gif"), "-vf", f"select=eq(n\\,{index - 1})",
                "-frames:v", "1", str(frame),
            )
            if not frame.is_file():
                raise RuntimeError(f"Missing frame {index} in {name}.gif")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--locale", choices=("zh", "en"), action="append", help="defaults to both")
    parser.add_argument("--skip-build", action="store_true", help="use an existing probe build")
    parser.add_argument("--preview", type=Path, default=Path("/tmp/readme-capture-preview"))
    parser.add_argument("--stage-only", type=Path, help="write review assets here instead of assets/shots")
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("the native capture probe requires macOS")
    if not args.skip_build:
        run(
            "cargo", "build", "--offline", "--locked", "--manifest-path", "apps/native/Cargo.toml",
            "--target-dir", str(TARGET), "--example", "workspace_wiring_probe",
        )
    locales = args.locale or ("zh", "en")
    with tempfile.TemporaryDirectory(prefix="readme-capture-", dir=TARGET) as temp:
        scratch = Path(temp)
        for locale in locales:
            folder = scratch / locale
            folder.mkdir()
            frames = record(locale, folder)
            output = folder / "output"
            encode(frames, output)
            extract(output, args.preview / locale)
        for locale in locales:
            if args.stage_only:
                destination = args.stage_only / locale
            else:
                destination = ROOT / "assets/shots" / ("en" if locale == "en" else "")
            destination.mkdir(parents=True, exist_ok=True)
            for file in (scratch / locale / "output").iterdir():
                shutil.copyfile(file, destination / file.name)
            print(f"Saved {locale} assets to {destination}", flush=True)


if __name__ == "__main__":
    main()
