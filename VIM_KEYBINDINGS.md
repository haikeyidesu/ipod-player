# Vim-Style Keybindings for iPod Player

## Navigation Keys (Vim-style)

| Key | Action | Description |
|-----|--------|-------------|
| `j` / `↓` | Down | Move selection down in menu |
| `k` / `↑` | Up | Move selection up in menu |
| `h` / `←` / `Esc` | Left/Back | Go back to menu from open page |
| `l` / `→` / `Enter` | Right/Open | Open selected menu item |
| `g` / `G` | First/Last | Jump to the first/last item, across pages |

## Playlist song selection

In a saved playlist's options, choose **Select Music** to mark songs from the
entire paginated library. This picker does not affect the active queue.

| Key | Picker action |
| --- | --- |
| `Space` | Toggle focused song and move down one row |
| `Shift+j` / `Shift+↓` or `Shift+k` / `Shift+↑` | Extend the selection from the anchor across pages |
| `a` | Select all library songs (background scan) |
| `A` | Invert selection across the whole library (background scan) |
| `Enter` / centre on a song | Toggle that song without moving |
| `Enter` / centre on **Add Selected** | Add marked songs to the saved playlist and show its updated songs |
| `h` / `Esc` / MENU | Discard the marks and return without adding songs |

The **Add Selected** row shows the marked-song count. A batch error keeps songs
that were not added selected, so retrying does not duplicate successful additions.
Outside this picker, Space is unbound; use the click-wheel play button or a media key for playback.

## MPD mixer volume

- `,` lowers and `.` raises the **MPD internal mixer** by 5 percentage points
  from any page; neither changes macOS system volume or handles hardware keys.
- Settings → Playback → Volume shows a single 0–100% slider. `j/k`, ↑/↓
  and wheel rotation adjust it by 5 points; mouse click/drag sets an exact value
  and commits once on release. MENU/`h` returns to Playback settings.
- If MPD has no software mixer, the control is disabled and displays
  “MPD mixer unavailable”. The playback page reflects external volume changes.

## Transport Controls

| Key | Action | Description |
|-----|--------|-------------|
| `[` | Previous | Restart after 3 seconds elapsed; otherwise previous track |
| `]` | Next | Next track |
| `n` | Next | Alias for next track |
| `p` | Previous | Alias for previous track |

## Click Wheel Navigation

The click wheel has four sections and a center button:

- **Top (MENU)** - Press to go back to menu
- **Left (│◀)** - Restart after 3 seconds elapsed; otherwise previous track
- **Right (▶│)** - Press to skip to next track  
- **Bottom (▶Ⅱ)** - Press to play/pause
- **Center** - Click to select/open menu item
- **Drag around the outer ring** - Scroll through lists

## Unified LCD selection and activation

- **Single-click any row**: select/highlight only, including settings and actions.
- **j/k, ↓/↑, mouse wheel or rotational click wheel**: move that same selection.
  Fine mouse-wheel/trackpad deltas accumulate before one menu step; click-wheel
  rotation keeps its existing response.
- **Double-click a row, Enter, l/→, or virtual centre button**: activate through
  the same dispatcher. Double-clicking selects the target and activates once;
  ordinary clicks never execute an action.
- Progress-bar dragging remains a direct continuous control. After switching
  workspaces, click the iPod to give its dockless window keyboard focus; merely
  viewing the pinned window does not take focus from the active application.

## Now Playing and Lyrics

| Key | Normal playback view | Carousel mode |
| --- | --- | --- |
| `h` / `←` / `Esc` / MENU | Back | Lyrics → playback → exit carousel |
| `l` / `→` / Enter / centre | Activate carousel | Keep carousel active |
| `j` | Seek backward 5 seconds | Next panel; scroll down in plain lyrics |
| `k` | Seek forward 5 seconds | Previous panel; scroll up in plain lyrics |
| `↓` / `↑` | Seek forward / backward 5 seconds | Next / previous panel; scroll plain lyrics |
| `n` / `]` | Next song | Next song |
| `p` / `[` | Restart after 3 seconds, otherwise previous | Same |
| Play button | Play/pause | Play/pause |

- Double-click the artwork/metadata area to activate the carousel with a mouse.
  Mouse-wheel or rotational-wheel scrolling then navigates the panels. Outside
  carousel mode, these scroll inputs seek directly, like j/k.
- Synced lyrics automatically follow MPD elapsed time, including paused seeks.
  Enter/centre (or double-click lyrics) toggles full-transcript browsing;
  j/k and wheel scroll while browsing. Enter/centre or MENU/h exits browsing
  and snaps back to the currently synced lyric. The next MENU/h returns to
  playback. Plain lyrics always scroll manually; MENU/h returns to playback.
- Long synced lines wrap fully, with a full scrollable transcript in browse mode.
  Selected LCD rows and long Now Playing title/artist labels slide horizontally
  after a short pause; unselected rows remain compact and elided.
- Click or drag the progress bar to preview; release commits once. MENU cancels.
  Seeking needs a positive duration and a playing or paused track. Track changes
  cancel a drag; seeking while paused never resumes playback.
- Transport buttons always transport, including during a drag (which is cancelled).
- Queue position remains the actual MPD queue position, not random playback order.

See [lyrics/cache behavior and validation](docs/LYRICS.md).

## Queue and Settings

- Collection **Play** replaces the active queue, disables MPD random/single mode,
  and starts at the first track. Collection **Shuffle** replaces it with a shuffled
  order and also plays sequentially through that order. Repeat is unchanged.
- Individual-song **Play Now** still appends and starts that song. **Play Next** and
  **Add to Queue** preserve the queue and playback settings.
- **Add to Playlist** changes a saved MPD playlist, not the active queue.
- **Settings → Playback** controls MPD volume (0–100%, not system volume) via a
  slider, native crossfade (Off, 1, 2, 3, 5, 10 seconds), repeat and random/shuffle mode. Status
  follows external changes. An unavailable mixer is shown explicitly.
- Seek steps use the named `seek-interval-seconds` property in `ui/app.slint`.

## macOS Shortcuts Preserved

The following macOS shortcuts are preserved and not intercepted:
- `Cmd` + any key
- `Ctrl` + any key  
- `Option` + any key

## Focus Management

The navigation focus scope is focused at startup and refocused when using the click wheel, so vim-style navigation is available without a preliminary click.
