# Native window preferences and LCD

Settings → Window contains **Always on Top [On/Off]** and **Reset Window Size**.
The normal bundle is a dockless `LSUIElement` agent app (verified with the
window's keyboard focus, native resizing/dragging and Control Center). Pinning
defaults to Off and uses AppKit's standard floating level plus
`NSWindowCollectionBehaviorCanJoinAllSpaces`. Other collection flags remain
untouched. Unpinning restores the original level and entire collection behavior.
Neither action recreates/activates or moves the window. Fullscreen-app overlays
are not enabled; no AeroSpace polling or workspace-triggered frame updates are used.

Geometry and pinning share this local, atomic JSON file (no MPD settings):

```text
~/Library/Application Support/io.github.haikeyidesu.ipod-player/window.json
```

Geometry is recorded in native AppKit points, including negative monitor origins.
The first launch is 420×640, with a 420:640 aspect ratio and 315×480–630×960 limits.
Slint receives the restored size before show; the existing RenderingSetup hook
applies the native frame before the backend's first-frame reveal. Valid geometry
is preserved; unavailable monitors or changed usable areas cause fitting/clamping
to a visible screen. Screen layout changes are also checked while running.
If a desktop is smaller than the minimum size, the minimum is retained and the
top edge stays reachable. Reset requests 420×640 at the current top-left, clamped
to the usable desktop; it leaves pinning and unrelated settings untouched.

A 250 ms observation timer writes only after 750 ms without a geometry change.
Pin/reset actions and orderly shutdown flush immediately. Corrupt settings fall
back to defaults; I/O failures are logged without disabling the window. Forced
termination can lose the latest move inside the debounce period.

`ui/lcd-texture.slint` encapsulates a soft cool backlight, faint RGB stripe
filter and edge falloff. It tiles three tiny transparent PNGs (5, 6 and 9 logical px)
with nearest-neighbour sampling for stable spacing at 75%, 100% and 150%; no
per-frame procedural drawing or CRT scanlines. Regenerate the assets with
`python3 scripts/generate-lcd-tiles.py`. The layer sits below existing Aqua
reflections and contains no input handlers. Its `strength` input is reserved
for future Appearance controls.

## Verification

```sh
cargo fmt --check
cargo check --locked
cargo test --locked
python3 scripts/bundle-macos.py --no-install
```

The last command builds the release bundle and verifies dependencies, plist and
ad-hoc signature without replacing the installed application.

The regular bundle now uses the same tested `LSUIElement` metadata as the
successful accessory build. No NSPanel conversion, activation-policy runtime
change, `CanJoinAllApplications`, or workspace polling is used. Quit any old
`iPod Player Sticky Test.app` instance before launching the normal bundle so
both copies do not compete for playback metadata.

## Native manual checklist

- [ ] Fresh preferences: window is 420×640 and pinning is Off.
- [ ] Pin On and switch repeatedly between workspaces on the same display.
      Player remains
      visible and floating at the exact same on-screen size/position, without
      stealing keyboard focus or joining the tiling layout. Switch apps too:
      player stays above ordinary windows. Toggle Off and switch again: player
      returns to ordinary workspace-specific stacking/visibility. No focus jump
      or window recreation.
- [ ] Move/resize (including all edges/corners), quit immediately, relaunch:
      position, size and pinning restore. Repeat after waiting one second.
- [ ] Save on a secondary display, quit, disconnect it, relaunch. Repeat with
      resolution/Dock changes and a display disconnect while running: controls
      remain visible and aspect ratio/minimum/maximum remain sensible.
- [ ] Reset from small/large sizes near screen edges: 420×640 where it fits,
      visible placement, pinning unchanged; dragging/resizing still work.
- [ ] At 75%, 100%, 150%: LCD texture is faint, text/highlights/marquees stay clear,
      glass reflections remain visible. Menus, Lyrics scrolling, carousel,
      wheel, progress/volume dragging and keyboard controls still receive input.
- [ ] MPD playback and macOS Control Center remain unchanged.
