# macOS app icon

Keep your editable Icon Composer source at `assets/icon/AppIcon.icon` (a directory
containing `icon.json` and `Assets/`). `scripts/bundle-macos.py` now uses full
Xcode's `ictool` to rasterize that source, then `sips` and `iconutil` to generate
a macOS 12-compatible ICNS **inside the temporary build directory**. The source
is never renamed or modified; the resulting `AppIcon.icns` is embedded in the
signed app bundle. This is a static rendition of Icon Composer's default macOS
appearance, not the newer dynamic system icon treatment.

```sh
python3 scripts/bundle-macos.py --no-install  # verify without replacing the app
python3 scripts/bundle-macos.py               # install after quitting the old app
```

Building from `.icon` requires **full Xcode** selected by `xcode-select -p`.
If you only have Xcode Command Line Tools, supply an existing binary file at
`assets/icon/AppIcon.icns` instead (remove or move `AppIcon.icon` first).
Do not rename a `.icon` directory to `.icns`: ICNS is a single binary file.
Without either source the bundle uses macOS's generic application icon.

To produce a fallback ICNS from a square 1024×1024 PNG, run from the repository
root:

```sh
mkdir -p target/AppIcon.iconset
SOURCE=/absolute/path/to/your-1024px-icon.png
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$SOURCE" --out "target/AppIcon.iconset/icon_${size}x${size}.png"
  double=$((size * 2))
  sips -z "$double" "$double" "$SOURCE" --out "target/AppIcon.iconset/icon_${size}x${size}@2x.png"
done
iconutil -c icns target/AppIcon.iconset -o assets/icon/AppIcon.icns
```

The included artwork was AI-generated and supplied for this project. Review its
rights and branding suitability before public distribution. Keep any replacement
artwork's source and license alongside the icon. Generated iconsets belong under
ignored `target/`, not in the bundle source tree.
