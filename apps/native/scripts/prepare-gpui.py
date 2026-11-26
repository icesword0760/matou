#!/usr/bin/env python3
"""Prepare pinned GPUI with Matou's platform adaptations."""
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
WINDOWS_PATCH = Path("patches/gpui-windows-ime.patch")
PINCH_PATCH = Path("patches/gpui-macos-pinch-magnify.patch")
TOUCHPAD_PATCH = Path("patches/gpui-windows-precision-touchpad.patch")
PATCHES = (FONT_PATCH, ATLAS_PATCH, WINDOWS_PATCH, PINCH_PATCH, TOUCHPAD_PATCH)
DESTINATION = ROOT / "vendor/gpui"
FONT_SOURCE = Path("src/platform/mac/text_system.rs")
ATLAS_SOURCE = Path("src/platform/mac/metal_atlas.rs")
PATCHED_SHA256 = {
    "src/platform/windows.rs": "91c7f49b891cd82aa09b64ff4fe4ba67fb0ac8e6bc23bd0e6628bdc2c1b2f50d",
    "src/platform/windows/touchpad.rs": "5a472a1a39606d7d985daa6534dabc4bb840df6d78af918d93091d9d626e9b2d",
    "Cargo.toml": "39fe2b5b8e078c2ee3919fd9868c11e596e45e4ded74786e3cde3f64c2ec1294",
    "src/text_system/line_wrapper.rs": "14dc21e1551608e7346ee408adc4ecf87f7de7503d01d5117fc71ffb0b9e7bac",
    "src/elements/text.rs": "0b98368c20454743a9ffe821fac1d2de2b9e1740c062fe6ff24faef9a9dabfe2",
    FONT_SOURCE.as_posix(): "784e9e1fa80eac701935e1689a2d693dae186489614c2b0ad0c42d0878dcd261",
    ATLAS_SOURCE.as_posix(): "362ddf60c6550e344a354fb6b6a977f2ff068bd442f3256bf5c81e535796459b",
    "src/platform.rs": "d9ee9dd3930b5fd8c6288744c24c80d58e2b6a173c558bc57144c7b799f5c464",
    "src/window.rs": "d619360f1351c96562334597eb411e6990d903fe174546c59e662f6f86e54f78",
    "src/interactive.rs": "172ba338ac2947c39512c8a69903b16b8bf5816012aaf95d91e67ec578cebbf1",
    "src/elements/div.rs": "0cd36b6b8c44c553a616e715eecc6fab5f3e25a8229f5c1013d5df693ef8319d",
    "src/platform/mac/events.rs": "56d64101a737b268e627b4451267a80242d1ea0b9c52ae2cdcf1a339ce0aa4b8",
    "src/platform/mac/window.rs": "05c3daafd80dc14df263902b679f07c2885961b47450f7c6d3fa4e181a54c873",
    "src/platform/windows/direct_write.rs": "e345de60957683a982c560a00f7d685b17d23b91e1c9b45c52f930a4e4c0f609",
    "src/platform/windows/events.rs": "09c11fa0440f32dc0d6da01088df6ea8f73f3f3eeb391b6fa9321447b37b0a6b",
    "src/platform/windows/platform.rs": "f56046900c173914859233aad5af10717ec115a701e545c905fe022f8de340a3",
    "src/platform/windows/window.rs": "b89e72e8f251e84f18c9a0d3c912716e45b5fc377538bee1176b526f8b6503fe",
}
SOURCES = tuple(map(Path, PATCHED_SHA256))
ADDED_SOURCES = {Path("src/platform/windows/touchpad.rs")}
ORIGINAL_SHA256 = {
    "src/platform/windows.rs": "7a568673ee16d8ad4f2147fa4df75b274bd36ba7b3dbd8f90088beb20cbb40d2",
    "Cargo.toml": "f5686c4367d640e41e57a53679b89417ddf82b11ef4a484b8d1e6134763f74fc",
    "src/text_system/line_wrapper.rs": "09d8a804c277a526034c5056a0dc2835f24e46c546bc5342436f58c147c58e91",
    "src/elements/text.rs": "945be8b602d797491e8c14a259142bbb14cfd98acb7b76615798acaeda43c059",
    ATLAS_SOURCE.as_posix(): "243326441589a0f942f4e1ed83283c5829457007a0d58e9ee13cc04540195428",
    "src/platform.rs": "7106499142d5416edaf44168da5c7dcd778c527398cb86e487131ba6689b94ff",
    "src/window.rs": "40d58470971ccd658ead4d4f788219658fb68b9486aff73e143835c09237ab18",
    "src/interactive.rs": "2c1b61bf1d6e999641a618810bf0d120236575e3e50f81884eae256d2fc56b57",
    "src/elements/div.rs": "5c227b7b562aa7471d2f3e78b599ac668c6915b54ec994ce981ce6e29a55e9a6",
    "src/platform/mac/events.rs": "c5fe96c1d637947d42bc17e68be2336cce44d3a9e24bd38840a682594641e87f",
    "src/platform/mac/window.rs": "c21da3de36159b51e62f02666b335359d5fff8c20ddf79c4fc9e3195047c3f35",
    "src/platform/windows/direct_write.rs": "8047a0130272466935edd5c8a57aa309d3a3efbffdfe7c175cc5b3082566820c",
    "src/platform/windows/events.rs": "ad86330b2c3d26342ce8c45759a07c8ac57007fecedc7dd7bf861dfdfa2727c7",
    "src/platform/windows/platform.rs": "81ca3093537e9320e2e8a28bc15d9883080546b96a685c0662158d853e090334",
    "src/platform/windows/window.rs": "e8d8ce5c589e72ed3d6b86d8008b8e1cc0d4c471c272fd8e6c6f38351a714493",
}
PREVIOUS_SHA256 = {
    "src/platform/windows/touchpad.rs": (
        "de130cbc964f307d8cddfe26950fc99719ae196322eccf2226737803bfb39312",
    ),
    "src/platform.rs": (
        "a7a8075608b61796ba7871aa118f4c2c3efab97c3bc3ec0553ad4de32547b24c",
    ),
    "src/elements/div.rs": (
        "3c9325667dfc3a3a2b59655c9f48d673bf425f571a0700bd05c74449129f2cc1",
    ),
    "src/window.rs": (
        "8e92531b9b6c3601f70a8401ddd6c79494e7ed7cf4f7de2ee9dde33f31274296",
        "8560e49d65f7fcec992d2309d1fbff1d32cecac9fe4f39aab77f20d51e0fc639",
        "d28f4524f3a3c269b98ab0fcb98ed28d0ec8f20b837692e7de9b8efc9c1c2c50",
    ),
    "src/platform/windows/events.rs": (
        "ee294c71a1324eda335786e9448b061a0c86e7abf858535715ef8dc38f0d3c1b",
        "01f741c6822215d75d76d46d3066bc0386d5d6f5667ee4fb8a6d877d27d223a5",
        "13d2d19bc906335c7cf25e420be76577f536f5bf6e77f600cd1ca96013d1c7ca",
    ),
    "src/platform/windows/window.rs": (
        "eb47662f55c3bd920c161c19bac52032b98bfb3ff6af9ebed9ad2127dbe83a03",
        "b946befe60d271dc97dcad562da3bd70415bd4d138eaa5c0f1f53e516de25cfa",
        "30e3c2ceb3dad569157874d99852d6a44b0b2cb3880a5a7d7abc11fdcdc7c736",
        "2709270b8ac9683398fbaca82083196b8be343d69062dfa2b57c44fc6c09befd",
    ),
}
MARKER = ".matou-patch.json"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def current_sources():
    """Recognize all affected files before writing any part of an existing tree."""
    try:
        hashes = {}
        for source in SOURCES:
            try:
                hashes[source.as_posix()] = digest(DESTINATION / source)
            except FileNotFoundError:
                if source not in ADDED_SOURCES:
                    raise
                hashes[source.as_posix()] = None
    except OSError as error:
        raise SystemExit(f"Unrecognized GPUI source tree; preserve it before preparing: {error}")
    for name, source_hash in hashes.items():
        if (source_hash not in (PATCHED_SHA256[name], ORIGINAL_SHA256.get(name)) and
                source_hash not in PREVIOUS_SHA256.get(name, ())):
            raise SystemExit(f"The generated GPUI source {name} has local changes; preserve it before preparing again.")
    return hashes


def main():
    patches = {patch.as_posix(): digest(ROOT / patch) for patch in PATCHES}
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
        # Earlier markers may cover fewer files. Independently recognize all
        # controlled sources against the pinned original/patched bytes.
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
                if (previous.get("patchSha256") != patches[FONT_PATCH.as_posix()] or
                        previous.get("sourceSha256") != PATCHED_SHA256[FONT_SOURCE.as_posix()] or
                        digest(DESTINATION / FONT_SOURCE) != previous["sourceSha256"]):
                    raise SystemExit("The legacy GPUI font adaptation has local changes; preserve them before preparing it again.")
            elif previous.get("formatVersion") != 2:
                raise SystemExit("Unknown GPUI preparation marker format; preserve it before preparing it again.")
        before = current_sources()
        if previous is not None and previous.get("formatVersion") == 2:
            previous_sources = previous.get("sources")
            if not isinstance(previous_sources, dict) or not previous_sources or not all(
                source_hash is not None and before.get(path.replace("\\", "/")) == source_hash
                for path, source_hash in previous_sources.items()
            ):
                raise SystemExit("The generated GPUI source differs from its marker; preserve it before preparing it again.")
        if previous == record:
            print(f"GPUI {VERSION}: pinned platform adaptations already prepared")
            return

    cargo_root = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    cached = next((cargo_root / "registry/cache").glob(f"*/gpui-{VERSION}.crate"), None)
    with tempfile.TemporaryDirectory(prefix="matou-gpui-") as directory:
        temporary = Path(directory)
        archive = cached or temporary / f"gpui-{VERSION}.crate"
        if cached is None:
            if os.environ.get("CARGO_NET_OFFLINE", "").lower() == "true":
                raise SystemExit(f"GPUI {VERSION} archive missing from the offline Cargo cache")
            url = f"https://static.crates.io/crates/gpui/gpui-{VERSION}.crate"
            with urllib.request.urlopen(url, timeout=60) as response:
                archive.write_bytes(response.read())
        if digest(archive) != ARCHIVE_SHA256:
            raise SystemExit("GPUI archive does not match the pinned crates.io checksum")
        with tarfile.open(archive) as package:
            package.extractall(temporary, filter="data")
        prepared = temporary / f"gpui-{VERSION}"
        # Apply from the extracted archive even when TMPDIR is inside a worktree.
        # Otherwise git apply can discover that worktree and skip patch paths.
        patch_env = {**os.environ, "GIT_CEILING_DIRECTORIES": str(temporary.resolve())}
        # Preserve the archive's line endings for the pinned patched hashes.
        git_apply = ["git", "-c", "core.autocrlf=false", "apply"]
        for patch in PATCHES:
            subprocess.run([*git_apply, "--check", str(ROOT / patch)], cwd=prepared, env=patch_env, check=True)
            subprocess.run([*git_apply, str(ROOT / patch)], cwd=prepared, env=patch_env, check=True)
        actual = {source.as_posix(): digest(prepared / source) for source in SOURCES}
        if actual != PATCHED_SHA256:
            raise SystemExit("GPUI patch results differ from the known adaptations")
        if existing:
            # Do not replace the vendor tree: unrelated files and local work stay.
            if current_sources() != before:
                raise SystemExit("GPUI sources changed during preparation; no source files were replaced")
            for source in SOURCES:
                if before[source.as_posix()] != actual[source.as_posix()]:
                    shutil.copyfile(prepared / source, DESTINATION / source)
        else:
            DESTINATION.parent.mkdir(parents=True, exist_ok=True)
            shutil.copytree(prepared, DESTINATION, dirs_exist_ok=True)
        (DESTINATION / MARKER).write_text(json.dumps(record, indent=2) + "\n")
        print(f"GPUI {VERSION}: prepared verified platform adaptations")


if __name__ == "__main__":
    main()
