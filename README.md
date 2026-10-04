# iPod Player

A classic iPod-inspired macOS desktop controller for **Music Player Daemon (MPD)**,
built with Rust and Slint. MPD plays the audio; this app supplies the interface.

[![Made with Slint](docs/assets/made-with-slint.png)](https://slint.dev)

Independent project; not affiliated with or endorsed by Apple Inc.

## Preview

**Screenshot placeholder:** a clean Finder-launched Now Playing screenshot will
be added here. See [screenshot guidance](docs/screenshots/README.md). Local visual
test renders remain under ignored `target/now-playing-preview/`; they are not
published with personal music artwork.

## Features

- Frameless Aqua/glass iPod window, proportional scaling and native macOS resizing.
- Dockless native window with persistent pinning/geometry, Reset Window Size and a faint LCD texture;
  [window preferences and verification](docs/WINDOW.md).
- Click-wheel, keyboard and mouse navigation; classic Now Playing artwork/progress.
- Browse MPD artists, albums, songs, saved playlists and indexed folders.
- Collection Play/Shuffle replaces the queue; Play Next/Add to Queue preserve it.
- Song actions include adding to existing saved MPD playlists.
- Clear Queue preserves the current song; stable queue IDs back queue actions.
- Direct five-second seeking, draggable progress bar and consistent Vim-style controls.
- Vertical Now Playing/Lyrics carousel with synced LRCLIB lyrics, plain fallback
  and a persistent offline cache; [lyrics behavior and privacy](docs/LYRICS.md).
- Single-click row selection; unified double-click, Enter/l/Right and centre activation.
- Selectable/draggable MPD mixer-volume slider, global `,`/`.` volume shortcuts,
  crossfade, repeat and random mode (never macOS system volume).
- macOS Now Playing / Control Center metadata and transport, synchronized with MPD
  (including external changes from rmpc).

See [keyboard and seeking controls](VIM_KEYBINDINGS.md).

## Requirements

### Running the application

- macOS. Bundle metadata targets **macOS 12 or later**; older supported versions
  still need real-device testing. Current builds are native architecture, not universal.
- An **existing, separately installed MPD server**. MPD 0.24.x is recommended;
  MPD 0.23+ relative queue insertion is used. Artwork requires `albumart` and/or
  `readpicture`. Some servers/tracks may provide no artwork.
- MPD must have music indexed and a working, enabled audio output.

The app does **not** bundle, launch, configure or silently install MPD.

### Building

- Rust/Cargo (edition 2024 capable; verified with Rust 1.98.1).
- Xcode or Xcode command-line tools, including the macOS SDK, Swift, `codesign`,
  `otool` and `install_name_tool`. MediaPlayer builds a static Swift bridge using
  Swift Package Manager; Swift tools 5.9+ are needed.
- Python 3 for packaging. Initial Cargo dependency downloads need internet access.

```sh
xcode-select --install           # only if developer tools are not installed
rustup update stable
cargo fmt --check
cargo check --locked
cargo test --locked
MACOSX_DEPLOYMENT_TARGET=12.0 cargo build --release --locked
python3 scripts/bundle-macos.py
```

The one-command bundle build is **`python3 scripts/bundle-macos.py`**. It
runs the release build itself, embeds UI/fonts/static resources, adds metadata and
licenses, validates dependencies and applies an **ad-hoc signature for local use**.
It also copies the verified bundle to **`/Applications/iPod Player.app`**.
Use `--no-install` to build and verify without replacing the installed application.
It does not install additional tools, mutate MPD or push Git commits.
The script resolves the repository path from its own location, so it also works
when called from another working directory.

Output with the default Cargo target directory:

```text
target/bundle/macos/iPod Player.app
  Contents/
    Info.plist
    PkgInfo
    MacOS/ipod-player
    Resources/
      LICENSE
      THIRD_PARTY_NOTICES.md
      licenses/
      AppIcon.icns              # optional, when supplied
```

Custom `CARGO_TARGET_DIR` is respected. Existing generated bundles are preserved
as `.previous-<timestamp>` siblings. See [custom icon instructions](assets/icon/README.md).
No final icon is supplied yet; the generic macOS icon is expected.

## Install and launch locally

Quit the running app **before rebuilding**, so the script can safely replace its
installed copy and two processes do not compete for Control Center ownership.
The build installs the app automatically in the system `/Applications` folder (write permission required);
previous installed copies are preserved as timestamped `.previous-*` siblings:

```sh
python3 scripts/bundle-macos.py
open "/Applications/iPod Player.app"
```

You can also double-click the installed app in Finder. Cargo, Terminal, source files
and local font files are **not needed at runtime**. Gatekeeper may warn for builds
downloaded from the internet until Developer ID signing/notarization is completed;
ad-hoc signing is not a public distribution identity. Use normal macOS security
approval for a trusted local build—do not disable Gatekeeper globally.

## MPD setup

Finder does not inherit your shell’s environment. With a normal Finder launch the
app defaults to **`127.0.0.1:6600`**. Keep your existing MPD configuration; this
project ships no personal music paths or local server config.

A typical macOS MPD output block is:

```conf
bind_to_address "127.0.0.1"
port "6600"
audio_output {
    type "osx"
    name "CoreAudio"
    mixer_type "software"
}
```

This is a snippet, not a complete MPD configuration. Configure your own music,
database and state paths according to MPD’s documentation. Validate actual audio
with `mpc status`, `mpc outputs` and `mpc volume` before blaming the UI. A FIFO
visualizer output can advance playback even when CoreAudio fails. Check MPD logs,
macOS output selection and mute state before restarting or editing configuration.

Developer launches may override `MPD_HOST` and `MPD_PORT`, for example:

```sh
MPD_HOST=127.0.0.1 MPD_PORT=6600 cargo run
```

The current client supports TCP endpoints, not Unix sockets or `password@host`
authentication. Custom Finder server preferences are not implemented. If MPD is
unavailable the window still opens; metadata clears and library queries show an
error. For diagnostics, run the copied executable directly from a terminal:

```sh
"/Applications/iPod Player.app/Contents/MacOS/ipod-player"
```

## Release verification and distribution

See [macOS release checklist](docs/RELEASING.md) for bundle inspection, independent
launch checks, Developer ID signing, notarization and GitHub release guidance.
No prebuilt notarized release is promised yet.

## License

Project code: [MIT](LICENSE). Fonts and dependencies retain their licenses; see
[third-party notices](THIRD_PARTY_NOTICES.md). Slint is used under its royalty-free
desktop license. Keep the attribution badge prominently visible on public download
pages (including GitHub Releases) when distributing under that license.
