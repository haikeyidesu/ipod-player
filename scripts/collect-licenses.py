#!/usr/bin/env python3
"""Collect upstream crate license files without publishing Cargo's local paths."""
import json
import pathlib
from pathlib import Path
import shutil
import sys


def collect(metadata, destination):
    destination = pathlib.Path(destination)
    destination.mkdir(parents=True, exist_ok=True)
    inventory = []
    missing = []
    for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
        if package['source'] is None:
            continue
        root = pathlib.Path(package['manifest_path']).parent
        target = destination / (package['name'] + '-' + package['version'])
        files = set()
        for path in root.iterdir():
            if path.name.lower().startswith(('license', 'licence', 'copying', 'notice', 'copyright')):
                if path.is_file():
                    files.add(path)
                elif path.is_dir():
                    files.update(p for p in path.rglob('*') if p.is_file())
        if package.get('license_file'):
            path = root / package['license_file']
            if path.is_file() and root in path.resolve().parents:
                files.add(path)
        for path in sorted(files):
            relative = path.relative_to(root)
            output = target / relative
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, output)
        supplements = Path(__file__).resolve().parent.parent / 'licenses' / 'rust-overrides' / (package['name'] + '-' + package['version'])
        supplemental_files = []
        if supplements.is_dir():
            for path in sorted(supplements.iterdir()):
                if path.is_file():
                    target.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(path, target / path.name)
                    supplemental_files.append(path.name)
        if not files and not supplemental_files:
            missing.append(package['name'] + '-' + package['version'])
        inventory.append({'name': package['name'], 'version': package['version'],
                          'license': package.get('license'), 'repository': package.get('repository'),
                          'license_files': [str(p.relative_to(root)) for p in sorted(files)],
                          'upstream_supplements': supplemental_files})
    (destination / 'inventory.json').write_text(json.dumps(inventory, indent=2) + '\n')
    if missing:
        (destination / 'REVIEW-MISSING-LICENSES.txt').write_text(
            'These packages do not ship standalone license files; review their source notices before distribution.\n'
            + '\n'.join(missing) + '\n')
        print('License review needed for: ' + ', '.join(missing), file=sys.stderr)
    return missing


if __name__ == '__main__':
    with open(sys.argv[1]) as source:
        collect(json.load(source), sys.argv[2])
