#!/usr/bin/env python3
"""Prepare the pinned GPUI crate with Matou's macOS font adaptation."""
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
PATCH = ROOT / "patches/gpui-macos-synthetic-italic.patch"
DESTINATION = ROOT / "vendor/gpui"
SOURCE = Path("src/platform/mac/text_system.rs")
MARKER = ".matou-patch.json"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    patch_sha = digest(PATCH)
    previous = None
    if (DESTINATION / MARKER).exists():
        previous = json.loads((DESTINATION / MARKER).read_text())
        if digest(DESTINATION / SOURCE) != previous["sourceSha256"]:
            raise SystemExit("The generated GPUI source has local changes; preserve them before preparing it again.")
        if previous["archiveSha256"] == ARCHIVE_SHA256 and previous["patchSha256"] == patch_sha:
            print(f"GPUI {VERSION}: pinned adaptation already prepared")
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
        subprocess.run(["git", "apply", "--check", str(PATCH)], cwd=prepared, check=True)
        subprocess.run(["git", "apply", str(PATCH)], cwd=prepared, check=True)
        record = {
            "version": VERSION,
            "archiveSha256": ARCHIVE_SHA256,
            "patchSha256": patch_sha,
            "sourceSha256": digest(prepared / SOURCE),
        }
        # A first development preparation may already contain the exact patch.
        # Do not replace an unrecognized tree or overwrite local source edits.
        if DESTINATION.exists():
            if digest(DESTINATION / SOURCE) == record["sourceSha256"]:
                (DESTINATION / MARKER).write_text(json.dumps(record, indent=2) + "\n")
                print(f"GPUI {VERSION}: verified existing adaptation")
                return
            if previous is None:
                raise SystemExit("Unrecognized GPUI directory; preserve it before preparing the pinned dependency")
            shutil.rmtree(DESTINATION)
        DESTINATION.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(prepared, DESTINATION)
        (DESTINATION / MARKER).write_text(json.dumps(record, indent=2) + "\n")
        print(f"GPUI {VERSION}: prepared verified source and macOS font patch")


if __name__ == "__main__":
    main()
