# Native window preferences and LCD

Settings → Window contains **Always on Top [On/Off]**, **Edge Tuck [On/Off]**
(default Off), and **Reset Window Size**.
The normal bundle is a dockless `LSUIElement` agent app (verified with the
window's keyboard focus, native resizing/dragging and Control Center). Pinning
defaults to Off and uses AppKit's standard floating level plus
`NSWindowCollectionBehaviorCanJoinAllSpaces`. Other collection flags remain
untouched. Unpinning restores the original level and entire collection behavior.
Pinning does not recreate/activate or move the window. Fullscreen-app overlays
are not enabled; no AeroSpace polling or workspace-triggered frame updates are used.

Geometry, pinning and the Edge Tuck preference share this local, atomic JSON file
(no MPD settings):

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

With Edge Tuck enabled, dragging a free window partly **through** an outer
left, right or bottom edge (but less than one-third of its size) snaps it fully
onscreen, still **open and docked**. Releasing a free window flush with the edge
or inside it—even a few pixels away—does not dock. Pushing one-third or more
through the edge tucks directly instead. AppKit may constrain the native drag,
but the unconstrained pointer gesture determines the mode; there is no gap or
overlap at the threshold. With Edge Tuck Off, partial overshoots simply return
fully onscreen. Shared boundaries with another monitor are excluded so the
hidden body cannot spill onto that monitor. A deep edge drop animates directly
from the release position to the tucked frame, leaving a 24-point sliver with
a slim, centered grey handle; it does not first snap back to the fully shown
position. Hovering does not reveal it: click the slit to slide open over about
300 ms and focus, or hold and drag the slit along the current edge to move it
while tucked. The cursor becomes an open hand on hover, closes during a drag
and returns to the arrow on exit or reveal. A drag never pulls the hidden tab
out from the edge. The bottom handle is centered in the visible content strip.
While revealed, the matching grey handle on the edge-facing side clicks to
re-tuck (except during playlist naming). While moving a shown dock, its frame
stays flush with the current edge and slides parallel to it, even if the
pointer strays inward. Pull about 100 points inward to detach it; after that,
it moves freely without a 100-point jump. Once revealed, the window re-tucks
about 200 ms after the pointer leaves, even if still key; held mouse buttons,
recent keyboard
activity, scrubbing and playlist naming defer hiding. A newly open dock stays
open until the pointer has entered its shown window, so releasing outside the
screen cannot immediately hide it. Turning Edge Tuck Off or resetting the
window reveals/undocks immediately. Pinning remains independent. Only the
fully shown native frame is saved; relaunching does not reopen half offscreen.
A changed display layout restores the shown window and undocks it; drag to dock
again on an available edge. The preference is persisted, but docking itself is
session-local. AppKit tracking handles the tab; a short timer animates the
existing native window frame only during click-to-reveal. Slint layers are not
translated and there is no additional idle polling loop.

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

- [ ] Fresh preferences: window is 420×640; pinning and Edge Tuck are Off.
- [ ] With Edge Tuck Off, drop the iPod partly outside each edge: it returns
      fully onscreen. A small visible gap from an edge stays unchanged.
- [ ] Turn Edge Tuck On; park flush or a few pixels inside an edge: no dock.
      Drop partly beyond each outer edge: it should snap back fully shown but
      stay docked, without immediately hiding if the pointer lands outside.
      Test just below and at one-third overshoot: only at/above the boundary
      should it tuck directly, leaving the 24-point marked tab without first
      snapping into full view. Hover should leave it tucked;
      click the slit to watch the full slide open and focus; click the shown
      edge-facing handle to hide again. Drag the hidden slit along each edge
      without revealing, including at screen ends and near shared monitor edges.
      While shown, drag parallel and pull slightly inward: the window should
      stay flush **during** movement; pull inward 100 points to detach without
      jumping. Check open/closed-hand cursors, the arrow on exit, and the
      centered bottom handle. Relaunch to check that the new shown position
      persists. Confirm continuous keyboard use, held mouse drags and playlist
      naming keep it open.
      Repeat while pinned; drag away, resize, reset and toggle Off. Repeat with a
      second monitor and after disconnecting that monitor.
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
