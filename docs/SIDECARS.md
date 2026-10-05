# Local lyrics (Phase 1)

## Configure a music root

Set **`IPOD_MUSIC_DIR` to an absolute, locally accessible music directory** matching
MPD's database root. No mpd.conf discovery, guessing, network share mounting or
music-library scanning is performed. A remote MPD requires a locally mounted copy
with the same relative paths. Start the app with that environment, for example:

```sh
IPOD_MUSIC_DIR="/Volumes/Music" cargo run --locked
# Or launch the bundled executable directly with the same environment:
IPOD_MUSIC_DIR="/Volumes/Music" \
  "target/bundle/macos/iPod Player.app/Contents/MacOS/ipod-player"
```

Finder launches do not inherit shell exports. Restart after changing the root.
Do not commit a personal path to the repository. An unset root preserves the old
provider behavior; a configured but inaccessible/missing root fails closed with a
logged error and no lyrics, without affecting playback. It is checked again on
subsequent reload polls, so remounting the directory recovers automatically.

For MPD URI `Artist/Album/01 Song.flac`, the sidecar is:

```text
<IPOD_MUSIC_DIR>/Artist/Album/01 Song.lrc
```

The audio extension is **replaced**, not appended (`Song.lrc`, not `Song.flac.lrc`).
Matching is by exact path, not title/artist. Same-stem audio files in one directory
share one sidecar. Only lowercase `.lrc` is looked up (filesystem case rules apply).
Unicode/spaces/literal percent signs are retained; URI text is never URL-decoded.
Absolute paths, URLs, traversal, empty components, backslashes and colon-bearing
URIs are rejected. Audio must exist as a regular file. Symlinks below the configured
root, including a symlink sidecar, are rejected; the root itself is canonicalized.

The root must be user-controlled. These checks prevent URI traversal and ordinary
symlink escapes, but are not a capability sandbox against another process replacing
ancestor directories concurrently. Do not export into an untrusted concurrently
modified filesystem tree. Editing/replacing the sidecar itself atomically is fine.

## Authority and reload

- Local files win over all provider cache/network results, even when empty.
- UTF-8 (optional BOM), CRLF, timestamps, offsets and repeated timestamps work.
  Untimed UTF-8 text uses the existing plain-lyrics presentation.
- Existing unreadable, invalid UTF-8, non-regular or oversized (>2 MiB) files
  suppress provider fallback; errors are logged, not silently hidden.
- Read-only music roots work for playback. Normal playback **never writes** lyrics.
- The worker rereads only the active sidecar about every two seconds, including
  during pause; external edits, creation and deletion are detected without restart.
  It does not scan the library or perform disk I/O on the UI thread. A blocking
  provider request may delay reload until that request completes (up to its timeout).
- After deletion, the provider cache/lookup is used. Its result or error is reused
  until a new track request, not re-fetched on every reload tick. Track changes still
  clear lyrics and reject stale asynchronous results via generation tickets.
- Save external edits atomically to avoid displaying an editor's partial write.

## Non-destructive v1 export

Existing `lyrics-v1` JSON remains a disposable **provider cache**, not an editable
source of truth. Nothing is moved, deleted or auto-promoted into the music folder.
Select an individual hashed JSON file from:

```text
~/Library/Application Support/io.github.haikeyidesu.ipod-player/lyrics-v1/
```

Preview the export (no UI, MPD connection, network lookup or write):

```sh
IPOD_MUSIC_DIR="/Volumes/Music" cargo run --locked -- \
  --export-lyrics "/absolute/path/to/lyrics-v1/<hash>.json"
```

Review the destination, then repeat with `--write` to create it. The installed
binary accepts the same arguments. No bulk migration or automatic choice between
multiple cached versions of a recording is made. The operator selects the exact
cache entry; preview and inspect its metadata before writing.

Export validates v1/hash identity, requires a real audio path and exports only
suitable synced lyrics. Plain, missing, instrumental, malformed, unsorted,
non-finite/negative or over-24-hour timestamps and tag-like lyric text are rejected.
Timestamps are rounded to milliseconds. Translations use repeated timestamps.
Because v1 stored parsed lines, original LRC formatting/tags/offset declarations
cannot be recovered; the already-applied timing is retained in normalized LRC.
No audio metadata or file contents are changed.

The new UTF-8 file is fully written and fsynced in the destination directory before
an atomic **no-replace hard-link publication**. Existing files, directories and
symlinks are never overwritten, including a destination created after preview.
Temporary files are cleaned up on ordinary errors. Read-only/full filesystems and
filesystems without hard-link support fail safely (no unsafe copy/rename fallback).
A process crash may leave a `.ipod-lyrics-*.tmp` file; originals remain untouched.
Directory entries are not explicitly fsynced, so sudden power-loss durability is
filesystem-dependent. Cached originals are always retained.

These are ordinary portable LRC files, independent of this app's JSON format.
Other clients must support/configure this sidecar convention; this pass does not
configure rmpc or claim that every rmpc version automatically discovers sidecars.

## Boundaries and verification

`local.rs` owns confined path resolution, UTF-8 loading and no-clobber publication.
`resolver.rs` owns precedence and the per-track fallback memo. A supplied fallback
closure isolates provider/cache work without introducing a plugin framework.
`cache.rs` owns disposable provider data and reading selected v1 exports.
`parser.rs` is shared by sidecars and provider results. Slint presentation is
unchanged. Future providers/editing can reuse these boundaries; neither is added.

Tests cover traversal/URL/symlink rejection, Unicode paths, BOM/offset parsing,
precedence including invalid/empty files, edits/deletion, creation during a lookup,
provider-error memoization, export round-trip/identity, unchanged cache originals,
read-only directories and atomic overwrite refusal. Native/manual follow-up:
configure a real root, edit a sidecar while paused, export a reviewed cache entry,
and verify the LRC with your other client. Tests never modify personal music.
