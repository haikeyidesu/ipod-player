# Third-party notices

The application code is MIT licensed; see [LICENSE](LICENSE). Dependencies and
assets retain their own licenses. Apple/iPod names are trademarks of Apple Inc.;
this is an independent project, not an Apple product or endorsement.

## Slint

[Slint](https://slint.dev), copyright SixtyFPS GmbH and contributors, is used under
the **Slint Royalty-free Desktop, Mobile, and Web Applications License 2.0**.
The license is reproduced in `licenses/Slint-Royalty-free-2.0.md`.
The README displays the official Slint attribution badge. Keep that badge visible
on public download/release pages when distributing this desktop application under
this license. The project’s MIT license does not relicense Slint.

## Fonts (unmodified)

- **IBM Plex Sans Condensed Bold**: copyright © 2017 IBM Corp., Reserved Font Name
  “Plex”; SIL Open Font License 1.1. See `licenses/IBM-Plex-OFL.txt` and the original
  `assets/fonts/IBM_Plex_Sans_Condensed/OFL.txt`.
- **Symbols Nerd Font Mono**, Nerd Fonts Symbols Only v3.5.1: Nerd Fonts MIT
  notice, copyright © 2014 Ryan L McIntyre. See `licenses/Nerd-Fonts-MIT.txt`.
  The font incorporates multiple upstream icon sets under their respective
  licenses (MIT, SIL OFL, Apache 2.0, CC BY 4.0 and Unlicense). The original set
  list and attribution links are in
  `assets/fonts/Symbols_Nerd_Font_Mono/README.md`. Additional full license notices
  and source URLs are in `licenses/nerd-font-glyphs/`.
  The upstream README’s “unlicensed” label for Font Logos refers to the
  Unlicense/public-domain dedication, reproduced there.

Both imported fonts are embedded into the executable by Slint. Only those two
font files are tracked; unused local font variants and download archives are not
part of the project. No music, album artwork from the music library, or Apple app
icon is redistributed in this repository.

## Rust and native dependencies

`Cargo.lock` records the exact versions. The bundle command collects the license,
NOTICE and COPYING files supplied with resolved crates into
`Contents/Resources/licenses/rust-crates/`, with a path-free `inventory.json`
listing crate names, versions, license expressions and upstream repositories.
For registry archives missing license files, pinned upstream supplements are in
`licenses/rust-overrides/` with revision provenance in `SOURCES.md`. The current
remaining review item is `dispatch 0.2.0`: its manifest declares MIT, but neither
its archive nor pinned repository root provides a standalone license notice.
Resolve that notice before distributing public binaries.

This includes build-time packages conservatively; presence in the inventory does
not mean a crate is linked into the executable. If any crate omits standalone
license files, `REVIEW-MISSING-LICENSES.txt` lists it for release review.

The separate `mpd-now-playing` helper owns the MediaPlayer Swift bridge and
has its own dependency/license review. Apple frameworks and the system Swift
runtime remain OS dependencies, not vendored libraries.
Review dependency license obligations and any missing notices before public
binary distribution, especially when updating Cargo.lock. Generated inventory is
an aid to compliance, not a legal audit.
