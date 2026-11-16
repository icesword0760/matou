#!/usr/bin/env python3
"""Prepare pinned GPUI with Matou's macOS font and atlas adaptations."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
VERSION = "0.2.2"
ARCHIVE_SHA256 = "979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707"
FONT_PATCH = Path("patches/gpui-macos-synthetic-italic.patch")
ATLAS_PATCH = Path("patches/gpui-macos-atlas-retirement.patch")
PATCHES = (FONT_PATCH, ATLAS_PATCH)
DESTINATION = ROOT / "vendor/gpui"
FONT_SOURCE = Path("src/platform/mac/text_system.rs")
ATLAS_SOURCE = Path("src/platform/mac/metal_atlas.rs")
SOURCES = (FONT_SOURCE, ATLAS_SOURCE)
PATCHED_SHA256 = {
    str(FONT_SOURCE): "784e9e1fa80eac701935e1689a2d693dae186489614c2b0ad0c42d0878dcd261",
    str(ATLAS_SOURCE): "362ddf60c6550e344a354fb6b6a977f2ff068bd442f3256bf5c81e535796459b",
}
ORIGINAL_ATLAS_SHA256 = "243326441589a0f942f4e1ed83283c5829457007a0d58e9ee13cc04540195428"
MARKER = ".matou-patch.json"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def current_sources():
    """Recognize both affected files before writing any part of an existing tree."""
    try:
        hashes = {str(source): digest(DESTINATION / source) for source in SOURCES}
    except OSError as error:
        raise SystemExit(f"Unrecognized GPUI source tree; preserve it before preparing: {error}")
    if hashes[str(FONT_SOURCE)] != PATCHED_SHA256[str(FONT_SOURCE)]:
        raise SystemExit("The generated GPUI font source has local changes; preserve them before preparing it again.")
    if hashes[str(ATLAS_SOURCE)] not in (
        ORIGINAL_ATLAS_SHA256, PATCHED_SHA256[str(ATLAS_SOURCE)]
    ):
        raise SystemExit("The generated GPUI atlas source has local changes; preserve them before preparing it again.")
    return hashes


def main():
    patches = {str(patch): digest(ROOT / patch) for patch in PATCHES}
    record = {
        "formatVersion": 2,
        "version": VERSION,
        "archiveSha256": ARCHIVE_SHA256,
        "patches": patches,
        "sources": PATCHED_SHA256,
    }
    existing = DESTINATION.exists() and any(DESTINATION.iterdir())
    before = None
    if existing:
        # The old marker protects the font only. Validate that file first, then
        # independently recognize the atlas against pinned original/patched bytes.
        marker = DESTINATION / MARKER
        previous = None
        if marker.exists():
            try:
                previous = json.loads(marker.read_text())
            except (OSError, ValueError) as error:
                raise SystemExit(f"Unrecognized GPUI preparation marker: {error}")
            if (previous.get("version") != VERSION or
                    previous.get("archiveSha256") != ARCHIVE_SHA256):
                raise SystemExit("Unrecognized GPUI preparation marker; preserve this tree before preparing it again.")
            if "formatVersion" not in previous:
                if (previous.get("patchSha256") != patches[str(FONT_PATCH)] or
                        previous.get("sourceSha256") != PATCHED_SHA256[str(FONT_SOURCE)] or
                        digest(DESTINATION / FONT_SOURCE) != previous["sourceSha256"]):
                    raise SystemExit("The legacy GPUI font adaptation has local changes; preserve them before preparing it again.")
            elif previous.get("formatVersion") != 2:
                raise SystemExit("Unknown GPUI preparation marker format; preserve it before preparing it again.")
        before = current_sources()
        if previous is not None and previous.get("formatVersion") == 2:
            if previous.get("sources") != before:
                raise SystemExit("The generated GPUI source differs from its marker; preserve it before preparing it again.")
        if previous == record:
            print(f"GPUI {VERSION}: pinned font and atlas adaptations already prepared")
            return

    cargo_root = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    cached = next((cargo_root / "registry/cache").glob(f"*/gpui-{VERSION}.crate"), None)
    with tempfile.TemporaryDirectory(prefix="matou-gpui-") as directory:
        temporary = Path(directory)
        archive = cached or temporary / f"gpui-{VERSION}.crate"
        if cached is None:
            url = f"https://static.crates.io/crates/gpui/gpui-{VERSION}.crate"
            with urllib.request.urlopen(url, timeout=60) as response:
                archive.write_bytes(response.read())
        if digest(archive) != ARCHIVE_SHA256:
            raise SystemExit("GPUI archive does not match the pinned crates.io checksum")
        with tarfile.open(archive) as package:
            package.extractall(temporary, filter="data")
        prepared = temporary / f"gpui-{VERSION}"
        for patch in PATCHES:
            subprocess.run(["git", "apply", "--check", str(ROOT / patch)], cwd=prepared, check=True)
            subprocess.run(["git", "apply", str(ROOT / patch)], cwd=prepared, check=True)
        actual = {str(source): digest(prepared / source) for source in SOURCES}
        if actual != PATCHED_SHA256:
            raise SystemExit("GPUI patch results differ from the known adaptations")
        if existing:
            # Do not replace the vendor tree: unrelated files and local work stay.
            if current_sources() != before:
                raise SystemExit("GPUI sources changed during preparation; no source files were replaced")
            for source in SOURCES:
                if before[str(source)] != actual[str(source)]:
                    shutil.copyfile(prepared / source, DESTINATION / source)
        else:
            DESTINATION.parent.mkdir(parents=True, exist_ok=True)
            shutil.copytree(prepared, DESTINATION, dirs_exist_ok=True)
        (DESTINATION / MARKER).write_text(json.dumps(record, indent=2) + "\n")
        print(f"GPUI {VERSION}: prepared verified font and atlas adaptations")


if __name__ == "__main__":
    main()
