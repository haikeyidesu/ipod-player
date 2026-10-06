# Lyrics and Now Playing validation

## Lookup, privacy and offline behavior

Only the current track is resolved, never as a library-wide scan. This also
happens with the lyrics panel closed. Source precedence is:

1. Authoritative UTF-8 `.lrc` beside the audio file under `IPOD_MUSIC_DIR`.
2. Disposable per-track provider cache in Application Support.
3. GET `https://lrclib.net/api/get`, then `/api/search` only if needed.

A local file bypasses both cache and online lookup, including an empty local file.
Read/encoding/path errors fail closed (log + no lyrics), rather than silently
substituting online lyrics for maintained local work. Without `IPOD_MUSIC_DIR`,
the previous cache/provider behavior remains. See [SIDECARS.md](SIDECARS.md) for
configuration, reload, safe cache export and filesystem constraints.

The worker checks the active sidecar every two seconds, even when paused. It
publishes only changed results, without resetting manual browsing on every poll.
Provider results/errors are memoized per active track: local reload is not an HTTP
retry loop. A second local check after a provider lookup catches files saved while
the network request was in flight. Existing generation guards reject old tracks.
Synced/plain/instrumental/missing presentation is unchanged.

Title and artist must match (case/whitespace normalization only). Album must match
when known and duration must be within two seconds when known. Remix/live/cover
qualifiers are never stripped. Ambiguous releases/durations are rejected rather
than guessing. Conservative matching may intentionally miss some valid lyrics.
An accepted exact plain-text result avoids an unnecessary search request.

LRCLIB receives **title, artist, album and duration**, not MPD paths, server
credentials or audio files. Requests identify iPod Player and its project URL.
One worker makes sequential requests with a 500 ms gap, bounded body size and
connection/read timeouts. HTTP 429/503 sets a service-wide cooldown using
`Retry-After` (seconds or HTTP date). No automatic retry loop runs: revisit a
track after cooldown, or restart after connectivity returns. HTTP/network failures
are not permanently cached. Existing cache hits work during cooldown/offline.

Downloaded lyrics and instrumental results persist in:

```text
~/Library/Application Support/io.github.haikeyidesu.ipod-player/lyrics-v1/
```

Files use SHA-256 keys of structured track identity (MPD file identity, exact title,
artist, album, duration), verify that identity when read, and are atomically written
with user-only file permissions. Positive results do not expire; true no-match
results expire after 24 hours. Corrupt entries are ignored. If a cache write fails,
lyrics still display, but offline persistence is not guaranteed; the error is logged.
Deleting this directory clears the lyrics cache. Do not commit or distribute it:
lyrics retain their owners' rights, and the cache contains personal track metadata.

Audio contents are never read or modified by the lyrics service (file metadata is
checked for safe path resolution). Normal playback only reads sidecars; explicit
export can create a new sidecar but never replace one. Application Support stores
provider data only, not local edits. No tagging, editor or additional provider is
implemented in Phase 1.

## Synced rendering

Standard `[mm:ss]`, fractional seconds, multiple timestamps, metadata tags and
`[offset:milliseconds]` are supported. Lines are sorted; simultaneous translations
are merged. Empty timed lines mark instrumental gaps. Active selection uses the
latest timestamp not greater than `PlayerState.elapsed`; there is no independent
clock to drift while paused. MPD's existing roughly one-second playing poll remains
the timing resolution. Explicit seeks select the new line, including backward seeks.

The synced preview wraps the previous/current/next lines instead of eliding a
long current line. Press Enter/centre (or double-click) on Lyrics to browse the
full, wrapped transcript with j/k and mouse/virtual wheel. Enter/centre or MENU/h
leaves browse mode and immediately re-centres on the current playback line;
MENU/h again returns to Now Playing. Plain lyrics show the full transcript without
highlighting; Enter/centre/double-click enters the same bounded browse mode, and
exiting it retains the plain transcript's scroll position.
The Lyrics title and artist come from the existing PlayerState snapshot and remain
fixed above the clipped transcript, with the existing marquee for long text.
Manual scrolling uses wrapped text height and bounded logical-pixel offsets, not
lyric indices; playback keeps updating the active line without moving this viewport.
Scroll limits are live property bindings rather than changed-callback snapshots.
Long selected menu rows and song/artist labels in Now Playing and Lyrics
marquee in one direction, pausing briefly at the start of each lap.

Selectable menus stop at the first/last item, including at page boundaries.
All directional inputs share the browser controller; g/G still jump to the
first/last item. Lyrics, volume and seeking keep their existing boundaries.

Each track change immediately clears the displayed lyrics and advances a generation
counter. Old queued track requests are coalesced; old results cannot overwrite the current
song, even through an A → B → A change. Network and disk operations run off the UI
thread. Neither the MPD protocol nor its polling workers were replaced.

## Automated checks

```sh
cargo fmt --check
cargo check --locked
cargo test --locked
cargo run --example now_playing_preview
# Optional: one real lookup of LRCLIB's documented public sample, not your music:
cargo test live_lrclib_documented_example -- --ignored
```

Unit tests cover parsing, active-line selection, forward/backward seeks, pause,
missing/plain/synced results, strict candidate matching, Retry-After, cache identity,
corruption, expiry and stale async completion. The headless example is also a Cargo
test: it exercises actual Slint keys/pointer events, row selection/double-click,
centre/Enter/l/Right activation, carousel, wheel scrolling, transport and bar dragging.
It renders fictional fixture text at 75%, 100% and 150% under ignored `target/`.

## Manual checklist (native app + your MPD)

- [ ] Single-click Artists, an album, song, playlist, folder, settings row and
      Play Album: highlight only. Double-click each target: activate once.
- [ ] Select with mouse, move with j/k/arrows/wheel, activate with Enter/l/Right
      or centre. Confirm selection and focus stay consistent after page changes.
      In home/settings and multi-page song/queue lists, keep pressing down at
      the last item and up at the first: selection stays put with every input.
      No item should activate. Check g/G still jump to either end.
- [ ] Normal Now Playing: j seeks −5, k +5, h goes back, l/Enter enters carousel.
      p restarts above three seconds and goes previous at/below three seconds.
- [ ] In carousel: j/scroll down reveals Lyrics with a vertical transition;
      k returns for synced lyrics. Enter/centre enables browsing; j/k and wheel
      visibly scroll mixed short/multi-line lyrics independently of playback.
      Check both transcript boundaries (no wrapping), fixed title/artist, and
      long metadata marquees. Enter or MENU/h resumes current-line syncing;
      MENU/h again returns, then exits carousel.
- [ ] Verify actual synced lyrics before/at a timestamp, after forward/backward
      bar seeking, while paused and after resume. `n`/`>` skip next, `<`/`[` restart/go previous,
      `p` and the play button toggle playback, and `f`/`b` seek in both panels.
- [ ] Plain lyrics: Enter/centre/double-click to browse; scroll to both ends
      with keyboard/mouse/virtual wheel. Enter or h/MENU leaves browsing without
      losing the scroll position; another h/MENU returns to playback. Confirm a
      long synced line wraps without `…`, and selected menu rows plus Now Playing
      and Lyrics titles loop left with a short pause at the start of each lap and no reversal.
      Check loading, no-match and instrumental states.
- [ ] Change tracks quickly while loading: old lyrics never reappear on a new song.
- [ ] Let a track download lyrics, quit the app, disable internet while keeping
      local MPD available, restart and play that track: cached lyrics still appear.
      Try an uncached track: a clean no-lyrics state, no frozen UI.
- [ ] Resize at minimum/normal/maximum sizes, drag the native window, use Control
      Center transport and drag the progress bar. Artwork and focus remain intact.

Rebuild/install as described in README. Quit the existing app before replacing it;
the currently running installed copy does not pick up source changes automatically.
