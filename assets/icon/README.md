# macOS app icon

Place your original/licensed custom icon at `assets/icon/AppIcon.icns`, then run
`python3 scripts/bundle-macos.py` again. The script checks the ICNS file header and
sets CFBundleIconFile. Without this file the app is valid but uses macOS’s generic
application icon. No Apple icon is copied or substituted.

To produce ICNS from a square 1024×1024 PNG (with transparency if desired), run
these commands from the repository root:

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

Commit AppIcon.icns and the icon’s source/license when you have a final design.
Generated iconsets belong under ignored `target/`, not in the bundle source tree.
