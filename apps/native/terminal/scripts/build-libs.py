#!/usr/bin/env python3
"""Build pinned libghostty-vt with the frozen Matou character-width policy.

Requires Zig 0.17.x (pass --zig). Release sources/dependencies are downloaded
and verified; original source is left intact in the ignored build cache.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parent.parent
VERSION = '1.0.1'
BASE = f'https://release.files.ghostty.org/libghostty-vt/{VERSION}'
ARCHIVES = {
    f'libghostty-vt-{VERSION}.tar.gz': '333dfce97641ef9b536bb9e08907ad86abaaada50e8f7417b70d9284adef37aa',
    f'libghostty-vt-{VERSION}-deps.tar.gz': '91a4e10960a52bf5bf4b15aa2a2cddaff8a89d6cd903d3055bda4fd17de3356d',
}
TARGETS = {
    'aarch64-apple-darwin': ('aarch64-macos', 'macos-aarch64'),
    'x86_64-apple-darwin': ('x86_64-macos', 'macos-x86_64'),
    'x86_64-pc-windows-msvc': ('x86_64-windows-gnu', 'windows-x86_64'),
    'aarch64-pc-windows-msvc': ('aarch64-windows-gnu', 'windows-aarch64'),
}


def digest(path):
    with path.open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', choices=TARGETS,
                        default=('aarch64' if platform.machine().lower() in ('arm64', 'aarch64') else 'x86_64')
                        + ('-apple-darwin' if platform.system() == 'Darwin' else '-pc-windows-msvc'))
    parser.add_argument('--zig', default='zig')
    parser.add_argument('--jobs', type=int, default=4)
    args = parser.parse_args()
    zig = shutil.which(args.zig)
    if not zig:
        parser.error('Zig 0.17.x is required; pass --zig /absolute/path/to/zig')
    zig = str(Path(zig).resolve())
    zig_version = subprocess.check_output([zig, 'version'], text=True).strip()
    if not zig_version.startswith('0.17.'):
        parser.error(f'Expected Zig 0.17.x, found {zig_version}')
    cache = ROOT / 'vendor' / 'build-cache'
    cache.mkdir(parents=True, exist_ok=True)
    for name, checksum in ARCHIVES.items():
        archive = cache / name
        if not archive.exists():
            partial = archive.with_suffix('.download')
            subprocess.run(['curl', '--fail', '--location', '--silent', '--show-error',
                            '--retry', '2', '--output', str(partial), f'{BASE}/{name}'], check=True)
            if digest(partial) != checksum:
                raise ValueError(f'{name}: checksum mismatch')
            partial.rename(archive)
        if digest(archive) != checksum:
            raise ValueError(f'{name}: checksum mismatch')
        unpacked = cache / name.removesuffix('.tar.gz')
        if not unpacked.exists():
            with tarfile.open(archive) as file:
                file.extractall(cache, filter='data')
    original = cache / f'libghostty-vt-{VERSION}'
    source = cache / f'libghostty-vt-{VERSION}-matou'
    if not source.exists():
        shutil.copytree(original, source)
    width = ROOT / 'native' / 'reference_width.zig'
    shutil.copy2(width, source / 'src/unicode/reference_width.zig')
    props = (original / 'src/unicode/props_uucode.zig').read_text()
    needle = '.width = uucode.get(.width, cp),'
    if props.count(needle) != 1:
        raise ValueError('Pinned Unicode generator changed')
    (source / 'src/unicode/props_uucode.zig').write_text(
        props.replace(needle, '.width = @import("reference_width.zig").get(cp),'))
    target, package_name = TARGETS[args.target]
    package = ROOT / 'vendor/ghostty-vt' / package_name
    env = dict(os.environ, ZIG_GLOBAL_CACHE_DIR=str(cache / 'zig-global'))
    subprocess.run([zig, 'build', '--system', str(cache / f'libghostty-vt-{VERSION}-deps'),
                    '-Doptimize=ReleaseFast', '-Dcpu=baseline', f'-Dtarget={target}',
                    f'-j{args.jobs}', '--prefix', str(package)], cwd=source, env=env, check=True)
    if 'apple-darwin' in args.target:
        # A distinct archive name prevents the macOS linker choosing the
        # upstream sibling dylib when linking this crate's own test executable.
        (package / 'lib/libghostty-vt.a').replace(package / 'lib/libmatou-ghostty-vt.a')
    shutil.copy2(original / 'LICENSE', package / 'LICENSE')
    shutil.copy2(ROOT / 'native/xterm-LICENSE', package / 'xterm-LICENSE')
    (package / 'matou-build.json').write_text(json.dumps({
        'version': VERSION, 'target': args.target, 'zig': zig_version,
        'sources': ARCHIVES, 'widthPolicy': 'xterm.js 6.0.0 / Unicode 6',
        'widthSha256': digest(width),
    }, indent=2) + '\n')
    print(f'Built Matou libghostty-vt at {package}')


if __name__ == '__main__':
    main()
