# Vim-Style Keybindings for iPod Player

## Navigation Keys (Vim-style)

| Key | Action | Description |
|-----|--------|-------------|
| `j` / `↓` | Down | Move selection down in menu |
| `k` / `↑` | Up | Move selection up in menu |
| `h` / `←` / `Esc` | Left/Back | Go back to menu from open page |
| `l` / `→` / `Enter` | Right/Open | Open selected menu item |
| `.` | Repeat | Repeats last up/down selection step |
| `g` / `G` | First/Last | Jump to the first/last item, across pages |

## Transport Controls

| Key | Action | Description |
|-----|--------|-------------|
| `[` | Previous | Restart after 3 seconds elapsed; otherwise previous track |
| `]` | Next | Next track |
| `Space` | Play/Pause | Toggle play/pause |
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

## Now Playing: Seeking

- **Center / Enter**: enter scrubbing; press again to commit.
- **h / ← / Previous**: restart current song after 3 seconds elapsed, otherwise
  previous track; while scrubbing, preview five seconds backward instead.
- **l / → / Next**: next track; while scrubbing, preview five seconds forward instead.
- **Drag the wheel on Now Playing**: automatically begin a preview; clockwise
  seeks forward, counterclockwise backward. Release to commit once.
- **j / ↓** while scrubbing: seek backward five seconds.
- **k / ↑** while scrubbing: seek forward five seconds.
- **Play button / Space** while scrubbing: commit and exit (like Center / Enter).
  Otherwise they retain normal play/pause behavior. Menu j/k navigation is unchanged.
- **MENU / Esc**: cancel without seeking.
- **Click or drag the bar**: preview the mouse position; release to commit once.
- The thin Aqua bar brightens and shows a small marker during preview. Times
  show the preview position while MPD continues playback normally.
- Seeking requires a known positive duration and a playing or paused track.
  A changed track cancels the preview. Committing a paused seek does not resume it.
- Existing transport keys still work. Queue position is the actual MPD queue
  position (not random-mode playback order).

## Queue and Settings

- Collection **Play** replaces the active queue, disables MPD random/single mode,
  and starts at the first track. Collection **Shuffle** replaces it with a shuffled
  order and also plays sequentially through that order. Repeat is unchanged.
- Individual-song **Play Now** still appends and starts that song. **Play Next** and
  **Add to Queue** preserve the queue and playback settings.
- **Add to Playlist** changes a saved MPD playlist, not the active queue.
- **Settings → Playback** controls MPD volume (0–100%, not system volume), native
  crossfade (Off, 1, 2, 3, 5, 10 seconds), repeat and random/shuffle mode. Status
  follows external changes. An unavailable mixer is shown explicitly.
- Seek steps use the named `seek-interval-seconds` property in `ui/app.slint`.

## macOS Shortcuts Preserved

The following macOS shortcuts are preserved and not intercepted:
- `Cmd` + any key
- `Ctrl` + any key  
- `Option` + any key

## Focus Management

The navigation focus scope is focused at startup and refocused when using the click wheel, so vim-style navigation is available without a preliminary click.
