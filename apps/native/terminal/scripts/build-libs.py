#!/usr/bin/env python3
"""Build pinned libghostty-vt with Matou's character widths and history trimming counter.

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
HISTORY_TRIM_PATCH_VERSION = 2
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


def patch_source(original, source, path, replacements):
    text = (original / path).read_text()
    for before, after in replacements:
        if text.count(before) != 1:
            raise ValueError(f'Pinned source {path} changed near {before.strip().splitlines()[0]!r}')
        text = text.replace(before, after)
    (source / path).write_text(text)


def patch_history_trim(original, source):
    pages = 'src/terminal/PageList.zig'
    terminal = 'src/terminal/c/terminal.zig'
    lifecycle = 'src/terminal/Terminal.zig'
    patch_source(original, source, lifecycle, [
        ('screens: ScreenSet,\n', 'screens: ScreenSet,\n\n/// Changes that invalidate external selection state.\nmatou_selection_epoch: usize = 0,\n'),
        ('    self.screens.switchTo(key);\n', '    self.screens.switchTo(key);\n    self.matou_selection_epoch +%= 1;\n'),
        ('pub fn fullReset(self: *Terminal) void {\n', 'pub fn fullReset(self: *Terminal) void {\n    self.matou_selection_epoch +%= 1;\n'),
    ])
    patch_source(original, source, pages, [
        (
            'total_rows: usize,\n\n/// The list of tracked pins.',
            'total_rows: usize,\n\n/// History prefix rows removed from this page list.\n'
            'matou_history_trimmed: usize = 0,\n\n/// The list of tracked pins.',
        ),
        (
            '            break :prune;\n        }\n\n'
            '        // If we have a pin viewport cache then we need to update it.',
            '            break :prune;\n        }\n\n'
            '        self.matou_history_trimmed += first.rows();\n\n'
            '        // If we have a pin viewport cache then we need to update it.',
        ),
        (
            '            pagelist.total_rows -= first_rows;\n',
            '            pagelist.total_rows -= first_rows;\n'
            '            pagelist.matou_history_trimmed += first_rows;\n',
        ),
        (
            '    self.eraseRows(.{ .history = .{} }, bl_pt);\n',
            '    const before = self.total_rows;\n'
            '    self.eraseRows(.{ .history = .{} }, bl_pt);\n'
            '    self.matou_history_trimmed += before - self.total_rows;\n',
        ),
    ])
    patch_source(original, source, terminal, [
        (
            '    memory_usage = 42,\n',
            '    memory_usage = 42,\n    matou_history_trimmed = 0x4d540001,\n'
            '    matou_selection_epoch = 0x4d540002,\n',
        ),
        (
            '            .total_rows,\n            .scrollback_rows,\n',
            '            .total_rows,\n            .scrollback_rows,\n'
            '            .matou_history_trimmed,\n            .matou_selection_epoch,\n',
        ),
        (
            '        .total_rows => out.* = t.screens.active.pages.total_rows,\n',
            '        .total_rows => out.* = t.screens.active.pages.total_rows,\n'
            '        .matou_history_trimmed => out.* = t.screens.active.pages.matou_history_trimmed,\n'
            '        .matou_selection_epoch => out.* = t.matou_selection_epoch,\n',
        ),
    ])
    return {
        'version': HISTORY_TRIM_PATCH_VERSION,
        'files': {path: digest(source / path) for path in (pages, terminal, lifecycle)},
    }


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
    patch_source(original, source, 'src/unicode/props_uucode.zig', [
        ('.width = uucode.get(.width, cp),', '.width = @import("reference_width.zig").get(cp),'),
    ])
    history_trim_patch = patch_history_trim(original, source)
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
        'historyTrimPatch': history_trim_patch,
    }, indent=2) + '\n')
    print(f'Built Matou libghostty-vt at {package}')


if __name__ == '__main__':
    main()
