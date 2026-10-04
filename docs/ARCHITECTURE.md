# Architecture and engineering audit

## Baseline

Checkpoint: `21a3ab87d1160c71f8928d61df3c93a84cf37f1b` (existing application).
On the local macOS toolchain, `cargo fmt --check`, `cargo check --locked` and
`cargo test --locked` pass: 63 binary tests + 3 headless UI tests; one opt-in
LRCLIB test ignored. Test execution: 0.54 s / 13.02 s respectively (debug,
warm build; UI tests include deliberate sleeps, not a performance benchmark).
Strict all-target Clippy reports ten existing style/iterator warnings.

## Responsibilities and data flow

- `main.rs`: composition, status-command channel, playback polling and snapshot
  application. One worker serializes transport/seek/volume commands. Poll delay
  is one second playing and five seconds paused/stopped, plus command refreshes.
  Changed snapshots enter Slint through `invoke_from_event_loop`; Control Center
  receives that same snapshot, not an independent MPD query.
- `mpd.rs`: TCP protocol, parsing, library queries and queue operations. No Slint
  dependencies. Two-second socket timeouts; large collection mutations allow
  120 seconds. Stable song IDs bind seeks and queue actions. Command lists are
  not rollback transactions: partial mutation failures require a status refresh.
- `library.rs`: navigation stack, six-row pages plus lookahead, generation-tagged
  fetches and serialized mutations. Stale completions cannot change a newer view.
  Song queries use MPD windows; counts accelerate end navigation. Tag/folder
  lists are fetched in full and paginated locally. The controller uses a mutex,
  though presentation changes execute on the event-loop thread.
- `artwork.rs`: sequential/coalesced track requests, off-thread download/decode,
  file-identity guard at delivery. Eight-entry LRU including misses; thumbnails
  at most 256×256 RGBA (about 2 MiB cache pixel storage), decode allocation limit
  64 MiB. These bounds exclude decoder overhead and temporary source buffers.
- `lyrics/`: HTTP matching, LRC parsing, atomic disk cache, UI presentation.
  Generation tickets reject stale completions including A→B→A. Cache/network work
  is off-thread; active-line selection is binary search on MPD elapsed time.
  LRCLIB requests/body sizes/timeouts are bounded; no library-wide fetching.
- `ui/`: shell and navigation dispatch in `app.slint`; wheel, Now Playing, lyrics,
  marquee, volume and passive texture/status components own presentation only.
  Rust supplies snapshots/models; Slint callbacks enqueue work rather than doing
  network I/O. The carousel retains both panels to preserve transition behavior.
- `platform/`: AppKit resize/focus/pinning adapter, battery subprocess, MediaPlayer
  bridge. AppKit ownership is main-thread-only. RAII releases native monitors and
  command tokens. `window_settings.rs` handles validated geometry and atomic,
  debounced persistence; native coordinates are not Slint-scaled coordinates.

## Prioritized findings (baseline source audit)

1. **Confirmed:** each ordinary playback snapshot opens two TCP connections,
   for `status` and `currentsong`; a mismatched song ID retries both. Batching can
   reduce handshakes without introducing persistent connection/replay semantics.
2. **Confirmed:** every changed elapsed-time snapshot invokes settings refresh,
   rebuilding visible settings models even when mixer/modes are unchanged.
3. **Confirmed:** lyrics candidate filtering, sorting and selection repeatedly
   parse the same LRC. Parse once per accepted candidate instead.
4. **Confirmed:** workers are detached; the library worker retains its own sender
   and cannot exit just through channel disconnection. In-flight requests have
   timeouts but orderly worker joining/cancellation is not implemented.
5. **Confirmed:** lyrics disk entries are individually capped at 2 MiB but total
   disk use is unbounded. Positive results deliberately persist for offline use;
   eviction would change that contract and needs a separately chosen policy.
6. **Confirmed:** artwork misses include transport errors and remain cached until
   LRU eviction; obsolete in-flight artwork can still download/decode. File guards
   protect display correctness, not wasted work. Lyrics already checks tickets
   before requests and delivery, but cannot interrupt an in-flight blocking HTTP call.
7. **Confirmed:** tag/folder lists and collection mutations materialize complete
   responses; MPD line-count bounds do not bound individual line bytes. Huge
   libraries deserve explicit workload tests before changing queue semantics.
8. **Potential cost, unmeasured:** retained hidden carousel marquees, gloss/LCD
   compositing, native screen observation at 4 Hz, synchronous settings fsync,
   full metadata publication and track-string allocation per playback tick.
   Do not remove effects or change animation lifecycle without visual/profiling tests.
9. **Reliability follow-up:** volume acknowledgements identify requests by value,
   not generation; repeated A→B→A values can acknowledge a newer pending request
   prematurely. Channel ordering limits but does not eliminate transient feedback.

## Implemented checkpoints and final verification

- `8b9fe47`: status and metadata now share one command-list connection. Separate
  `list_OK` boundaries prevent duration/field collisions; mismatched song IDs still
  retry once, then fail rather than publishing mixed-track state. No mutation is
  automatically replayed. A generic `Read + Write` stream seam permits an in-memory
  duplex protocol fixture without a mock framework or dependency.
- `f8ad79d`: settings models refresh only when volume/crossfade/repeat/random
  changes (including unavailable mixer transitions); elapsed-only ticks do not
  rebuild them. Lyrics candidates parse once, then select the first best-ranked
  result without sorting. Existing ambiguity and tie behavior is regression-tested.
- Five regression tests added; existing strict Clippy warnings resolved. No Slint,
  visual asset, dependency or native-platform behavior changes in this pass.

Structural before/after: ordinary snapshot TCP connections **2 → 1**; MPD data
commands remain **2 → 2**. Mismatch retry maximum connections **4 → 2**. These are
source/protocol-fixture counts, not measured network latency or CPU improvements.
Elapsed-only settings refresh callbacks **1 → 0 per changed snapshot**; candidate
LRC decoding **repeated → once per metadata-matching candidate**.

Final checks on rustc 1.98.1: fmt, locked check, locked tests and strict all-target
Clippy pass. **71 tests pass, one opt-in live LRCLIB test remains ignored**.
The macOS bundle script passes release compilation (8.12 s incremental), linked
library checks, plist validation and strict ad-hoc signature verification with
`--no-install`. Bundle: `target/bundle/macos/iPod Player.app`. Existing warnings:
no custom icon and `dispatch-0.2.0` license review needed before distribution.

A local TCP-listener test was blocked by sandbox permissions; it was replaced
with a deterministic in-memory duplex fixture. Real MPD interoperability and
native UI/Control Center manual checks were not run. No CPU/RSS/startup/latency
speedup is claimed. Worker shutdown, cache policy, volume request tickets and
native profiling remain follow-up work rather than unverified rewrites.

## Measurement and verification policy

No native CPU/RSS/startup or real-library latency baseline was collected. Running
an extra application can contend for Control Center and touch personal caches;
headless tests do not represent native GPU/MPD performance. Do not infer runtime
speedups from warm Cargo timings. Protocol command/connection counts can be tested
with local fixtures independently of a personal MPD server.

For follow-up profiling, use one release instance, a fixed MPD fixture/library,
fixed window size and renderer, and record machine/toolchain plus multiple runs:
startup-to-first-frame, idle/playing CPU and RSS, rapid track changes, page/end
navigation, large playlist mutations and resize. Compare identical workloads.

Verification: formatting, locked check/test, strict all-target Clippy, then
`python3 scripts/bundle-macos.py --no-install` (release build, dependency/plist/
signature verification without replacing the installed application). Native manual
checks remain in [WINDOW.md](WINDOW.md), [LYRICS.md](LYRICS.md) and
[RELEASING.md](RELEASING.md). Headless rendering tests protect controls and static
asset invariants but cannot validate Spaces, focus or Control Center visually.
