#!/usr/bin/env python3
"""Build a relocatable, ad-hoc-signed native-host macOS app. No MPD installation."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent


def run(*args, **kwargs):
    return subprocess.run(args, cwd=ROOT, check=True, **kwargs)


def add_icon(config, resources, stage):
    """Rasterize an Icon Composer source for our macOS 12-compatible bundle."""
    source = ROOT / config['icon']
    fallback = source.with_suffix('.icns')
    icon = resources / 'AppIcon.icns'
    if source.is_dir():
        developer = Path(run('xcode-select', '-p', capture_output=True, text=True).stdout.strip())
        ictool = developer.parent / 'Applications/Icon Composer.app/Contents/Executables/ictool'
        if not ictool.is_file():
            raise RuntimeError('AppIcon.icon requires full Xcode with Icon Composer (select it using xcode-select)')
        png = stage / 'icon-source.png'
        run(str(ictool), str(source), '--export-image', '--output-file', str(png),
            '--platform', 'macOS', '--rendition', 'Default', '--width', '1024',
            '--height', '1024', '--scale', '1')
        iconset = stage / 'AppIcon.iconset'
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                pixels = size * scale
                suffix = '@2x' if scale == 2 else ''
                output = iconset / f'icon_{size}x{size}{suffix}.png'
                run('/usr/bin/sips', '-s', 'format', 'png', '-z', str(pixels),
                    str(pixels), str(png), '--out', str(output), stdout=subprocess.DEVNULL)
        run('/usr/bin/iconutil', '-c', 'icns', str(iconset), '-o', str(icon))
    elif source.exists():
        raise RuntimeError(f'Icon Composer source must be a .icon directory: {source}')
    elif fallback.is_file():
        shutil.copy2(fallback, icon)
    elif fallback.exists():
        raise RuntimeError(f'Expected an ICNS file, not a directory: {fallback}')
    else:
        print('No AppIcon.icon or AppIcon.icns supplied; using the default macOS icon.')
        return False
    if icon.read_bytes()[:4] != b'icns':
        raise RuntimeError(f'Generated icon is not a valid ICNS file: {icon}')
    return True


def install(bundle, parent):
    """Stage a full copy before replacing an installed app; retain the old bundle."""
    parent.mkdir(parents=True, exist_ok=True)
    destination = parent / bundle.name
    if destination.is_symlink() or (destination.exists() and not destination.is_dir()):
        raise RuntimeError('Refusing to replace non-directory app destination: ' + str(destination))
    stage = Path(tempfile.mkdtemp(prefix='.stage-', dir=parent))
    backup = None
    try:
        candidate = stage / bundle.name
        shutil.copytree(bundle, candidate)
        run('/usr/bin/codesign', '--verify', '--deep', '--strict', str(candidate))
        if destination.exists():
            backup = parent / (bundle.name + '.previous-' + str(time.time_ns()))
            destination.rename(backup)
        try:
            candidate.rename(destination)
        except OSError:
            if backup is not None:
                backup.rename(destination)
            raise
        print('Installed: ' + str(destination))
        if backup is not None:
            print('Previous installed app: ' + str(backup))
    finally:
        shutil.rmtree(stage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--no-install', action='store_true', help='Build and verify only; do not replace the installed app')
    args = parser.parse_args()
    if sys.platform != 'darwin':
        raise SystemExit('This bundle script requires macOS and Xcode command-line tools.')
    host = next(line.split(': ', 1)[1] for line in run('rustc', '-vV', capture_output=True, text=True).stdout.splitlines() if line.startswith('host: '))
    metadata = json.loads(run('cargo', 'metadata', '--locked', '--format-version=1', '--filter-platform', host, capture_output=True, text=True).stdout)
    package = next(p for p in metadata['packages'] if Path(p['manifest_path']) == ROOT / 'Cargo.toml')
    config = package['metadata']['macos']
    env = os.environ.copy()
    env['MACOSX_DEPLOYMENT_TARGET'] = config['minimum-system-version']
    build = run('cargo', 'build', '--locked', '--release', '--bin', package['name'], '--message-format=json-render-diagnostics', env=env, capture_output=True, text=True)
    print(build.stderr, end='')
    artifacts = [json.loads(line) for line in build.stdout.splitlines() if line.startswith('{')]
    binary = next(Path(a['executable']) for a in artifacts if a.get('reason') == 'compiler-artifact' and a.get('executable') and a['target']['name'] == package['name'])
    parent = Path(metadata['target_directory']) / 'bundle' / 'macos'
    parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix='.stage-', dir=parent))
    try:
        bundle = stage / (config['name'] + '.app')
        contents = bundle / 'Contents'
        macos = contents / 'MacOS'
        resources = contents / 'Resources'
        macos.mkdir(parents=True)
        resources.mkdir()
        executable = macos / package['name']
        shutil.copy2(binary, executable)
        executable.chmod(0o755)
        info = {
            'CFBundleName': config['name'], 'CFBundleDisplayName': config['name'],
            'CFBundleIdentifier': config['identifier'], 'CFBundleExecutable': package['name'],
            'CFBundlePackageType': 'APPL', 'CFBundleInfoDictionaryVersion': '6.0',
            'CFBundleShortVersionString': package['version'],
            'CFBundleVersion': package['version'].split('-')[0],
            'LSMinimumSystemVersion': config['minimum-system-version'],
            'NSHighResolutionCapable': True,
            # A dockless agent app retains the pinned window across workspaces.
            # The NSWindow remains Slint-owned and can still receive keyboard input.
            'LSUIElement': True,
            'NSPrincipalClass': 'NSApplication',
            'NSHumanReadableCopyright': 'Copyright © 2026 haikeyidesu. MIT; dependencies retain their licenses.',
        }
        if add_icon(config, resources, stage):
            info['CFBundleIconFile'] = 'AppIcon.icns'
        with (contents / 'Info.plist').open('wb') as output:
            plistlib.dump(info, output)
        (contents / 'PkgInfo').write_bytes(b'APPL????')
        for name in ['LICENSE', 'THIRD_PARTY_NOTICES.md']:
            shutil.copy2(ROOT / name, resources / name)
        shutil.copytree(ROOT / 'licenses', resources / 'licenses')
        spec = importlib.util.spec_from_file_location('collect_licenses', ROOT / 'scripts' / 'collect-licenses.py')
        collector = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(collector)
        collector.collect(metadata, resources / 'licenses' / 'rust-crates')
        # Never ship an app that secretly needs the developer's Homebrew installation.
        linked = run('/usr/bin/otool', '-L', str(executable), capture_output=True, text=True).stdout
        for line in linked.splitlines()[1:]:
            library = line.strip().split(' (', 1)[0]
            if not library.startswith(('/System/Library/', '/usr/lib/', '@rpath/libswift')):
                raise RuntimeError('Unbundled non-system dependency: ' + library)
        # Remove any developer-toolchain rpaths from the bundled copy; keep
        # system runtime paths such as /usr/lib/swift if present.
        loads = run('/usr/bin/otool', '-l', str(executable), capture_output=True, text=True).stdout.splitlines()
        for index, line in enumerate(loads):
            if line.strip() == 'cmd LC_RPATH':
                path = loads[index + 2].strip().split(' ', 2)[1]
                if path.startswith(('/Users/', '/Applications/', '/Library/Developer/', '/opt/')):
                    run('/usr/bin/install_name_tool', '-delete_rpath', path, str(executable))
        run('/usr/bin/plutil', '-lint', str(contents / 'Info.plist'))
        run('/usr/bin/codesign', '--force', '--sign', '-', str(bundle))
        run('/usr/bin/codesign', '--verify', '--deep', '--strict', str(bundle))
        destination = parent / bundle.name
        if destination.exists():
            # Preserve the previous generated bundle rather than deleting it.
            destination.rename(parent / (bundle.name + '.previous-' + str(time.time_ns())))
        bundle.rename(destination)
        print('Bundle: ' + str(destination))
        if not args.no_install:
            install(destination, Path('/Applications'))
        print(linked)
        print('Local ad-hoc signing only. Developer ID signing/notarization is required for normal public distribution.')
    finally:
        shutil.rmtree(stage)


if __name__ == '__main__':
    try:
        main()
    except subprocess.CalledProcessError as error:
        print(error.stderr or str(error), file=sys.stderr)
        raise SystemExit(error.returncode)
