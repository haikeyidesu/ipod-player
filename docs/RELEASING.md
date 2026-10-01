# macOS releases

## Build and inspect

From the repository root:

```sh
cargo fmt --check
cargo check --locked
cargo test --locked
MACOSX_DEPLOYMENT_TARGET=12.0 cargo build --release --locked
python3 scripts/bundle-macos.py
APP="$PWD/target/bundle/macos/iPod Player.app"
plutil -p "$APP/Contents/Info.plist"
file "$APP/Contents/MacOS/ipod-player"
otool -L "$APP/Contents/MacOS/ipod-player"
codesign --verify --deep --strict --verbose=2 "$APP"
```

The bundler refuses unknown non-system dylibs rather than producing an app that
silently depends on Homebrew. It removes developer-toolchain rpaths from the copied
executable and retains the system Swift runtime rpath. It does not modify the
Cargo-built binary, source UI or native resizing. UI and fonts are embedded by
`build.rs`; MPD artwork is fetched at runtime, not bundled.

Review `Contents/Resources/licenses/rust-crates/inventory.json` and any
`REVIEW-MISSING-LICENSES.txt` before publishing binaries. Metadata generation may
include extra build-time crates conservatively. Generated files contain no local
Cargo paths, Git credentials, configuration or music.

## Independent Finder launch

Do not overwrite an installed running app. Quit it first. To test relocation into
a fresh location without touching an existing installation:

```sh
SOURCE_APP="$PWD/target/bundle/macos/iPod Player.app"
TEST_DIR="$(mktemp -d /tmp/ipod-app-test.XXXXXX)"
ditto "$SOURCE_APP" "$TEST_DIR/iPod Player.app"
cd /tmp
open "$TEST_DIR/iPod Player.app"
```

Manual checklist:

- Finder launches the copied app with no terminal or source-directory dependency.
- Fonts/icons, artwork fallback and Aqua UI render; native resizing still works.
- MPD at localhost:6600 connects and audio is audible (the bundle must not start MPD).
- Now Playing / Control Center metadata, pause/resume and next/previous work.
- Close/reopen from Finder; only one player process is running.
- Verify with MPD unavailable: a usable window, no silent MPD installation/startup.
- Test the declared minimum macOS version and each supported CPU architecture on
  real hardware before claiming support. A host build is not a universal binary.

For a stronger resource check, copy the app to a clean machine/account without the
checkout. Do not rename or delete your working source tree just to test this.

## Public binary distribution (not performed automatically)

The script uses **ad-hoc signing**. This supports local testing but does not provide
an Apple-trusted distribution identity. Before sharing downloads broadly:

1. Supply an original/licensed app icon (`assets/icon/AppIcon.icns`).
2. Complete the dependency license review and include required notices.
3. Use an Apple Developer account and a **Developer ID Application** certificate.
   Sign with hardened runtime and secure timestamp; test required entitlements
   (do not add permissive entitlements without evidence). No App Sandbox entitlement
   is added by the local bundler; Mac App Store packaging is a separate task.
4. Submit a ZIP made with `ditto -c -k --keepParent` to `xcrun notarytool submit`,
   wait for acceptance, then `xcrun stapler staple` the app and recreate the ZIP.
   Store notarization credentials in a keychain profile, never this repository.
5. Verify with `codesign --verify --deep --strict`, `xcrun stapler validate` and
   `spctl --assess --type execute --verbose=2` on the finalized app. Test on a clean
   Mac, including a quarantined download. Do not confuse ad-hoc verification with
   Gatekeeper approval or notarization.
6. Tag a tested source commit, attach the finalized archive and checksums to a
   GitHub Release, and include a visible Slint attribution badge on that download
   page. Link its exact source commit and build instructions.

Example release-page badge (keep visible near the download link):

```markdown
[![Made with Slint](https://raw.githubusercontent.com/haikeyidesu/ipod-player/main/docs/assets/made-with-slint.png)](https://slint.dev)
```

## Repository safety

Use explicit file lists when staging. Review `git diff --cached --stat` and
`git diff --cached` before committing. Never force-push to establish this project.
Before the first push, recheck remote refs and confirm the intended destination:

```sh
git remote -v
git log --oneline --decorate
git status --short
git ls-remote origin
# Only after user approval:
git push -u origin main
```

No CI credentials, local music, MPD state/configuration, signing certificates,
notarization tokens, build outputs, logs or generated bundles belong in Git.
