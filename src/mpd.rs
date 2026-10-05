//! Minimal Music Player Daemon client for reading player state.
//!
//! This module provides:
//! - Connection to MPD via TCP
//! - Status command parsing (title, artist, album, elapsed, duration, playing)
//! - Current song metadata retrieval
//!
//! Protocol I/O is isolated from Slint. Queue shuffling uses `rand`; callers
//! must run blocking operations on workers, never on the presentation thread.
use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};

const TIMEOUT: Duration = Duration::from_secs(2);
const SHUFFLE_COMMANDS: &str =
    "command_list_ok_begin\nclear\nadd \"\"\nshuffle\nplay 0\ncommand_list_end";

/// Represents the current playback state from MPD status command.
///
/// Fields are populated by parsing MPD's `status` response.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayerState {
    pub file: String,
    pub song_id: Option<u64>,
    /// Current track title (may be empty if no track loaded)
    pub title: String,
    /// Current track artist (may be empty)
    pub artist: String,
    /// Current track album (may be empty)
    pub album: String,
    /// Elapsed time in seconds since track start
    pub elapsed: f64,
    /// Total duration in seconds
    pub duration: f64,
    /// Whether playback is currently playing.
    pub playing: bool,
    /// True only for MPD's `pause` state (not when stopped).
    pub paused: bool,
    /// Mixer volume (0–100), or None when MPD has no software mixer.
    pub volume: Option<i32>,
    pub crossfade: f64,
    pub repeat: bool,
    pub random: bool,
    /// Zero-based current position in MPD's queue (also present while paused).
    pub queue_position: Option<usize>,
    pub queue_length: Option<usize>,
    /// Playlist version increments when the MPD queue is edited.
    pub queue_version: Option<u64>,
}

/// Represents metadata from MPD's currentsong command.
#[derive(Debug, Clone)]
pub struct SongInfo {
    pub file: String,
    pub id: Option<u64>,
    /// Track title
    pub title: String,
    /// Track artist
    pub artist: String,
    /// Track album
    pub album: String,
    /// Duration in seconds (0.0 if unknown)
    pub duration: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artist {
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub name: String,
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Song {
    pub file: String, // MPD database path; never use the display title as an identifier.
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
}

/// A queue row; IDs remain stable even if its position changes.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueSong {
    pub song: Song,
    pub id: u64,
    pub position: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Previous,
    Next,
    Play,
    Pause,
    PlayPause,
}

impl Transport {
    pub fn from_ui(action: &str) -> Option<Self> {
        match action {
            "previous" => Some(Self::Previous),
            "next" => Some(Self::Next),
            "play-pause" => Some(Self::PlayPause),
            "play" => Some(Self::Play),
            "pause" => Some(Self::Pause),
            _ => None,
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::Previous => "previous",
            Self::Next => "next",
            Self::Play => "play",
            Self::Pause => "pause 1",
            // MPD's pause command without an argument toggles pause/resume.
            Self::PlayPause => "pause",
        }
    }
}

fn endpoint() -> Result<(String, u16), String> {
    let host = std::env::var("MPD_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port = std::env::var("MPD_PORT")
        .unwrap_or_else(|_| "6600".into())
        .parse::<u16>()
        .map_err(|_| "MPD_PORT must be a port number".to_string())?;
    if host.is_empty() || host.contains('@') || host.starts_with('/') {
        return Err("Only TCP MPD_HOST values are supported for now".into());
    }
    Ok((host, port))
}

fn response(reader: &mut impl BufRead) -> Result<(), String> {
    // MPD may send informational lines before the final OK or ACK.
    for _ in 0..256 {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).map_err(|err| err.to_string())?;
        if bytes == 0 {
            return Err("MPD closed the connection before replying".into());
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line == "OK" {
            return Ok(());
        }
        if line.starts_with("ACK ") {
            return Err(format!("MPD rejected the command: {line}"));
        }
    }
    Err("MPD reply exceeded the simple client's limit".into())
}

fn send_on_stream(stream: TcpStream, action: Transport) -> Result<(), String> {
    stream
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|err| err.to_string())?;
    stream
        .set_write_timeout(Some(TIMEOUT))
        .map_err(|err| err.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut greeting = String::new();
    reader
        .read_line(&mut greeting)
        .map_err(|err| err.to_string())?;
    if !greeting.starts_with("OK MPD ") {
        return Err(format!("Not an MPD server: {}", greeting.trim()));
    }
    writeln!(reader.get_mut(), "{}", action.command()).map_err(|err| err.to_string())?;
    reader.get_mut().flush().map_err(|err| err.to_string())?;
    response(&mut reader)
}

/// Read MPD status (playback, elapsed, duration, and mixer volume).
/// Track metadata normally comes from `currentsong`; use `read_player_state`
/// for the combined snapshot.
pub fn read_status() -> Result<PlayerState, String> {
    let (host, port) = endpoint()?;
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|err| format!("Cannot resolve MPD host: {err}"))?;
    let mut last_error = "MPD host has no TCP addresses".to_string();

    for address in addresses {
        match TcpStream::connect_timeout(&address, TIMEOUT) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(TIMEOUT))
                    .map_err(|err| err.to_string())?;
                stream
                    .set_write_timeout(Some(TIMEOUT))
                    .map_err(|err| err.to_string())?;
                let mut reader = BufReader::new(stream);
                let mut greeting = String::new();

                if reader.read_line(&mut greeting).is_err() {
                    return Err("Failed to read MPD greeting".into());
                }
                if !greeting.starts_with("OK MPD ") {
                    return Err(format!("Not an MPD server: {}", greeting.trim()));
                }

                // Send status command
                writeln!(reader.get_mut(), "status").map_err(|err| err.to_string())?;
                reader.get_mut().flush().map_err(|err| err.to_string())?;

                // Read and parse response
                let state = parse_status_response(&mut reader)?;
                return Ok(state);
            }
            Err(err) => last_error = err.to_string(),
        }
    }
    Err(format!(
        "Cannot connect to MPD at {host}:{port}: {last_error}"
    ))
}

/// Parse the raw status response lines into a PlayerState struct.
///
/// MPD status response format (one field per line):
///   volume: 75
///   state: "play" or "pause"
///   songname: "track.mp3"
///   file: "path/to/track.mp3"
///   artist: "Artist Name"
///   album: "Album Name"
///   track: 1
///   duration: "0:03:45.12" (MM:SS.cc format)
///   percentageplayed: "42.5"
///   elapsed: "0:01:23.45" (MM:SS.cc format)
fn parse_status_response(reader: &mut impl BufRead) -> Result<PlayerState, String> {
    parse_status_response_until(reader, "OK")
}

fn parse_status_response_until(
    reader: &mut impl BufRead,
    terminator: &str,
) -> Result<PlayerState, String> {
    let mut state = PlayerState {
        file: String::new(),
        song_id: None,
        title: String::new(),
        artist: String::new(),
        album: String::new(),
        elapsed: 0.0,
        duration: 0.0,
        playing: false,
        paused: false,
        volume: None,
        crossfade: 0.0,
        repeat: false,
        random: false,
        queue_position: None,
        queue_length: None,
        queue_version: None,
    };

    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(|err| err.to_string())? == 0 {
            return Err("MPD closed the connection before replying".into());
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line == terminator {
            break;
        }
        if line == "OK" || line == "list_OK" {
            return Err("Unexpected MPD reply boundary".into());
        }
        if line.starts_with("ACK ") {
            return Err(format!("MPD rejected the command: {line}"));
        }
        let Some((key, value)) = line.split_once(": ") else {
            continue;
        };

        match key {
            "elapsed" => {
                if let Ok(elapsed) = parse_decimal_time(value) {
                    state.elapsed = elapsed;
                }
            }
            "duration" => {
                if let Ok(duration) = parse_decimal_time(value) {
                    state.duration = duration;
                }
            }
            "state" => {
                state.playing = value == "play";
                state.paused = value == "pause";
            }
            "volume" => {
                state.volume = value.parse::<i32>().ok().filter(|v| (0..=100).contains(v));
            }
            "xfade" => state.crossfade = parse_decimal_time(value).unwrap_or(0.0),
            "repeat" => state.repeat = value == "1",
            "random" => state.random = value == "1",
            "playlistlength" => state.queue_length = value.parse().ok(),
            "songid" => state.song_id = value.parse().ok(),
            "song" => state.queue_position = value.parse().ok(),
            "playlist" => state.queue_version = value.parse().ok(),
            "songname" | "file" => {
                // Extract just the filename from path if present
                let path = value.trim();
                if let Some(filename) = path.split('/').next_back() {
                    state.title = filename.to_string();
                } else {
                    state.title = path.to_string();
                }
            }
            "artist" => {
                state.artist = value.to_string();
            }
            "album" => {
                state.album = value.to_string();
            }
            _ => {}
        }
    }

    Ok(state)
}

/// Parse seconds with an optional fraction, or MM:SS / H:MM:SS, to seconds.
/// MPD's status elapsed/duration and currentsong duration are normally decimal seconds.
fn parse_decimal_time(s: &str) -> Result<f64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(0.0);
    }

    let parts: Vec<&str> = s.split(':').collect();
    let seconds = parts
        .last()
        .unwrap()
        .parse::<f64>()
        .map_err(|_| format!("Invalid seconds: {s}"))?;
    let total = match parts.len() {
        1 => seconds,
        2 => {
            parts[0]
                .parse::<f64>()
                .map_err(|_| format!("Invalid minutes: {s}"))?
                * 60.0
                + seconds
        }
        3 => {
            parts[0]
                .parse::<f64>()
                .map_err(|_| format!("Invalid hours: {s}"))?
                * 3600.0
                + parts[1]
                    .parse::<f64>()
                    .map_err(|_| format!("Invalid minutes: {s}"))?
                    * 60.0
                + seconds
        }
        _ => return Err(format!("Invalid time format: {s}")),
    };
    if !total.is_finite() || total < 0.0 {
        return Err(format!("Invalid time: {s}"));
    }
    Ok(total)
}

/// Open a fresh connection for a status/metadata pair. Failed reads are not
/// retained across polls; the next poll naturally attempts reconnection.
fn read_snapshot() -> Result<(PlayerState, SongInfo), String> {
    let (host, port) = endpoint()?;
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|err| format!("Cannot resolve MPD host: {err}"))?;
    let mut last_error = "MPD host has no TCP addresses".to_string();

    for address in addresses {
        match TcpStream::connect_timeout(&address, TIMEOUT) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(TIMEOUT))
                    .map_err(|err| err.to_string())?;
                stream
                    .set_write_timeout(Some(TIMEOUT))
                    .map_err(|err| err.to_string())?;
                let mut reader = BufReader::new(stream);
                let mut greeting = String::new();

                if reader.read_line(&mut greeting).is_err() {
                    return Err("Failed to read MPD greeting".into());
                }
                if !greeting.starts_with("OK MPD ") {
                    return Err(format!("Not an MPD server: {}", greeting.trim()));
                }

                return snapshot_on_connection(&mut reader);
            }
            Err(err) => last_error = err.to_string(),
        }
    }
    Err(format!(
        "Cannot connect to MPD at {host}:{port}: {last_error}"
    ))
}

/// Read both replies on one connection. `list_OK` preserves reply boundaries;
/// command lists reduce round trips but do not guarantee an atomic snapshot.
fn snapshot_on_connection<S: std::io::Read + Write>(
    reader: &mut BufReader<S>,
) -> Result<(PlayerState, SongInfo), String> {
    writeln!(
        reader.get_mut(),
        "command_list_ok_begin\nstatus\ncurrentsong\ncommand_list_end"
    )
    .map_err(|err| err.to_string())?;
    reader.get_mut().flush().map_err(|err| err.to_string())?;
    let state = parse_status_response_until(reader, "list_OK")?;
    let song = parse_song_response_until(reader, "list_OK")?;
    response(reader)?;
    Ok((state, song))
}

/// Parse the raw currentsong response into a SongInfo struct.
///
/// MPD currentsong response format:
///   title: "Track Name"
///   file: "path/to/track.mp3"
///   artist: "Artist Name"
///   album: "Album Name"
///   track: 1
///   disctotal: 1
///   duration: "0:03:45.12"
#[cfg(test)]
fn parse_song_response(reader: &mut impl BufRead) -> Result<SongInfo, String> {
    parse_song_response_until(reader, "OK")
}

fn parse_song_response_until(
    reader: &mut impl BufRead,
    terminator: &str,
) -> Result<SongInfo, String> {
    let mut info = SongInfo {
        file: String::new(),
        id: None,
        title: String::new(),
        artist: String::new(),
        album: String::new(),
        duration: 0.0,
    };

    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(|err| err.to_string())? == 0 {
            return Err("MPD closed the connection before replying".into());
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line == terminator {
            break;
        }
        if line == "OK" || line == "list_OK" {
            return Err("Unexpected MPD reply boundary".into());
        }
        if line.starts_with("ACK ") {
            return Err(format!("MPD rejected the command: {line}"));
        }
        let Some((key, value)) = line.split_once(": ") else {
            continue;
        };

        match key {
            "Id" => info.id = value.parse().ok(),
            "file" => {
                info.file = value.to_string();
                if info.title.is_empty() {
                    info.title = value.rsplit('/').next().unwrap_or(value).to_string();
                }
            }
            "Title" | "title" | "songname" => {
                info.title = value.to_string();
            }
            "Artist" | "artist" => {
                info.artist = value.to_string();
            }
            "Album" | "album" => {
                info.album = value.to_string();
            }
            "duration" => {
                if let Ok(duration) = parse_decimal_time(value) {
                    info.duration = duration;
                }
            }
            _ => {}
        }
    }

    Ok(info)
}

/// Fetch status and song metadata as one UI snapshot. Call only from a worker thread.
pub fn read_player_state() -> Result<PlayerState, String> {
    read_player_state_using(read_snapshot)
}

fn read_player_state_using(
    mut snapshot: impl FnMut() -> Result<(PlayerState, SongInfo), String>,
) -> Result<PlayerState, String> {
    let (mut state, mut song) = snapshot()?;
    // Avoid pairing the previous song's elapsed time with new metadata.
    if state.song_id != song.id {
        (state, song) = snapshot()?;
        if state.song_id != song.id {
            return Err("Track changed while reading state".into());
        }
    }
    state.file = song.file;
    state.title = song.title;
    state.artist = song.artist;
    state.album = song.album;
    if state.duration == 0.0 {
        state.duration = song.duration;
    }
    Ok(state)
}

/// Connect for one command; future clients can replace this behind Transport.
pub fn send(action: Transport) -> Result<(), String> {
    if action == Transport::Previous {
        let state = read_status()?;
        query(&previous_command(&state))?;
        return Ok(());
    }
    let (host, port) = endpoint()?;
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|err| format!("Cannot resolve MPD host: {err}"))?;
    let mut last_error = "MPD host has no TCP addresses".to_string();
    for address in addresses {
        match TcpStream::connect_timeout(&address, TIMEOUT) {
            Ok(stream) => return send_on_stream(stream, action),
            Err(err) => last_error = err.to_string(),
        }
    }
    Err(format!(
        "Cannot connect to MPD at {host}:{port}: {last_error}"
    ))
}

fn previous_command(state: &PlayerState) -> String {
    if state.elapsed > 3.0
        && (state.playing || state.paused)
        && let Some(id) = state.song_id
    {
        return format!("seekid {id} 0");
    }
    "previous".into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackSetting {
    Volume(u8),
    Crossfade(u8),
    Repeat(bool),
    Random(bool),
}
impl PlaybackSetting {
    fn command(self) -> Result<String, String> {
        Ok(match self {
            Self::Volume(v) if v <= 100 => format!("setvol {v}"),
            Self::Volume(_) => return Err("Volume must be 0–100".into()),
            Self::Crossfade(v) => format!("crossfade {v}"),
            Self::Repeat(v) => format!("repeat {}", u8::from(v)),
            Self::Random(v) => format!("random {}", u8::from(v)),
        })
    }
}
pub fn set_playback(setting: PlaybackSetting) -> Result<(), String> {
    if matches!(setting, PlaybackSetting::Volume(_)) && read_status()?.volume.is_none() {
        return Err("MPD mixer volume is unavailable".into());
    }
    query(&setting.command()?)?;
    Ok(())
}
fn playlist_add_command(name: &str, file: &str) -> Result<String, String> {
    Ok(format!("playlistadd {} {}", quote(name)?, quote(file)?))
}
pub fn add_to_playlist(name: &str, file: &str) -> Result<(), String> {
    let command = playlist_add_command(name, file)?;
    if !list_playlists()?.iter().any(|p| p.name == name) {
        return Err("Saved playlist no longer exists".into());
    }
    query(&command)?;
    Ok(())
}

/// Seek only the item for which the UI started its preview. Do not send play or
/// pause: MPD preserves the current playing/paused state when seeking this ID.
pub fn seek(id: u64, seconds: f64) -> Result<(), String> {
    let state = read_status()?;
    query(&seek_command(id, seconds, &state)?)?;
    Ok(())
}

fn seek_command(id: u64, seconds: f64, state: &PlayerState) -> Result<String, String> {
    if state.song_id != Some(id) || !(state.playing || state.paused) {
        return Err("Seek cancelled: playback item changed or stopped".into());
    }
    if !seconds.is_finite() || !state.duration.is_finite() || state.duration <= 0.0 {
        return Err("Seek requires a finite known duration".into());
    }
    // Stay just before the end, so previewing 100% does not skip to another song.
    let seconds = seconds.clamp(0.0, (state.duration - 0.001).max(0.0));
    Ok(format!("seekid {id} {seconds:.3}"))
}

/// Quote a single MPD argument. Reject line breaks so user-supplied names cannot
/// inject additional protocol commands.
fn quote(value: &str) -> Result<String, String> {
    if value.contains(['\n', '\r', '\0']) {
        return Err("Invalid MPD argument".into());
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

// Library queries use one short-lived connection, as the existing status client does.
fn query(command: &str) -> Result<Vec<(String, String)>, String> {
    query_with_timeout(command, TIMEOUT)
}

fn query_with_timeout(command: &str, timeout: Duration) -> Result<Vec<(String, String)>, String> {
    let (host, port) = endpoint()?;
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?;
    let mut last_error = "MPD host has no TCP addresses".to_string();
    for address in addresses {
        match TcpStream::connect_timeout(&address, TIMEOUT) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(timeout))
                    .map_err(|e| e.to_string())?;
                stream
                    .set_write_timeout(Some(timeout))
                    .map_err(|e| e.to_string())?;
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).map_err(|e| e.to_string())?;
                if !line.starts_with("OK MPD ") {
                    return Err("Not an MPD server".into());
                }
                writeln!(reader.get_mut(), "{command}").map_err(|e| e.to_string())?;
                reader.get_mut().flush().map_err(|e| e.to_string())?;
                return read_fields(&mut reader);
            }
            Err(e) => last_error = e.to_string(),
        }
    }
    Err(format!(
        "Cannot connect to MPD at {host}:{port}: {last_error}"
    ))
}

fn read_fields(reader: &mut impl BufRead) -> Result<Vec<(String, String)>, String> {
    let mut fields = Vec::new();
    let mut line = String::new();
    // Bound responses in case a server ignores 'window' or has a huge library.
    for _ in 0..100_000 {
        line.clear();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("MPD closed the connection before replying".into());
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line == "OK" {
            return Ok(fields);
        }
        if line.starts_with("ACK ") {
            return Err(format!("MPD rejected the command: {line}"));
        }
        if let Some((key, value)) = line.split_once(": ") {
            fields.push((key.to_string(), value.to_string()));
        }
    }
    Err("MPD library reply too large".into())
}

fn names(command: &str, field: &str) -> Result<Vec<String>, String> {
    let mut values: Vec<_> = query(command)?
        .into_iter()
        .filter_map(|(key, value)| (key == field && !value.is_empty()).then_some(value))
        .collect();
    values.sort_by_key(|s| s.to_lowercase());
    values.dedup();
    Ok(values)
}

pub fn list_artists() -> Result<Vec<Artist>, String> {
    Ok(names("list artist", "Artist")?
        .into_iter()
        .map(|name| Artist { name })
        .collect())
}
pub fn list_albums() -> Result<Vec<Album>, String> {
    Ok(names("list album", "Album")?
        .into_iter()
        .map(|name| Album { name })
        .collect())
}
pub fn albums_by_artist(name: &str) -> Result<Vec<Album>, String> {
    Ok(
        names(&format!("list album artist {}", quote(name)?), "Album")?
            .into_iter()
            .map(|name| Album { name })
            .collect(),
    )
}
pub fn list_playlists() -> Result<Vec<Playlist>, String> {
    Ok(names("listplaylists", "playlist")?
        .into_iter()
        .map(|name| Playlist { name })
        .collect())
}

fn songs_from_fields(fields: Vec<(String, String)>) -> Vec<Song> {
    let mut songs = Vec::new();
    let mut current: Option<Song> = None;
    for (key, value) in fields {
        match key.as_str() {
            "file" => {
                if let Some(song) = current.take() {
                    songs.push(song);
                }
                current = Some(Song {
                    title: value.rsplit('/').next().unwrap_or(&value).to_string(),
                    file: value,
                    ..Song::default()
                });
            }
            "Title" | "title" => {
                if let Some(song) = &mut current {
                    song.title = value;
                }
            }
            "Artist" | "artist" => {
                if let Some(song) = &mut current {
                    song.artist = value;
                }
            }
            "Album" | "album" => {
                if let Some(song) = &mut current {
                    song.album = value;
                }
            }
            "duration" => {
                if let Some(song) = &mut current {
                    song.duration = parse_decimal_time(&value).unwrap_or(0.0);
                }
            }
            _ => {}
        }
    }
    if let Some(song) = current {
        songs.push(song);
    }
    songs
}

fn page_range(offset: usize, limit: usize) -> Result<String, String> {
    if limit == 0 || limit > 100 {
        return Err("Song page size must be 1..=100".into());
    }
    let end = offset
        .checked_add(limit)
        .ok_or("Song page offset overflow")?;
    Ok(format!("{offset}:{end}"))
}

fn window(offset: usize, limit: usize) -> Result<String, String> {
    Ok(format!("window {}", page_range(offset, limit)?))
}

/// Page through the library; never request all song metadata at once.
pub fn list_songs(offset: usize, limit: usize) -> Result<Vec<Song>, String> {
    Ok(songs_from_fields(query(&format!(
        "search file \"\" {}",
        window(offset, limit)?
    ))?))
}
pub fn songs_by_artist(name: &str, offset: usize, limit: usize) -> Result<Vec<Song>, String> {
    Ok(songs_from_fields(query(&format!(
        "find artist {} {}",
        quote(name)?,
        window(offset, limit)?
    ))?))
}
pub fn songs_by_album(name: &str, offset: usize, limit: usize) -> Result<Vec<Song>, String> {
    Ok(songs_from_fields(query(&format!(
        "find album {} {}",
        quote(name)?,
        window(offset, limit)?
    ))?))
}
pub fn songs_by_album_and_artist(
    album: &str,
    artist: &str,
    offset: usize,
    limit: usize,
) -> Result<Vec<Song>, String> {
    Ok(songs_from_fields(query(&format!(
        "find album {} artist {} {}",
        quote(album)?,
        quote(artist)?,
        window(offset, limit)?
    ))?))
}
pub fn songs_by_playlist(name: &str, offset: usize, limit: usize) -> Result<Vec<Song>, String> {
    // listplaylistinfo accepts an optional range on supported MPD versions.
    Ok(songs_from_fields(query(&format!(
        "listplaylistinfo {} {}",
        quote(name)?,
        page_range(offset, limit)?
    ))?))
}

fn queue_from_fields(fields: Vec<(String, String)>) -> Vec<QueueSong> {
    fn entry(fields: Vec<(String, String)>) -> Option<QueueSong> {
        let id = fields.iter().find(|(key, _)| key == "Id")?.1.parse().ok()?;
        let position = fields
            .iter()
            .find(|(key, _)| key == "Pos")?
            .1
            .parse()
            .ok()?;
        let song = songs_from_fields(fields).into_iter().next()?;
        Some(QueueSong { song, id, position })
    }
    let mut entries = Vec::new();
    let mut fields_for_song = Vec::new();
    for (key, value) in fields {
        if key == "file"
            && !fields_for_song.is_empty()
            && let Some(song) = entry(std::mem::take(&mut fields_for_song))
        {
            entries.push(song);
        }
        fields_for_song.push((key, value));
    }
    if let Some(song) = entry(fields_for_song) {
        entries.push(song);
    }
    entries
}

/// Read only the requested slice of MPD's current queue.
pub fn queue_songs(offset: usize, limit: usize) -> Result<Vec<QueueSong>, String> {
    let range = page_range(offset, limit)?;
    Ok(queue_from_fields(query(&format!("playlistinfo {range}"))?))
}

/// Select an existing queue entry by stable ID; do not append another copy.
pub fn play_queue_id(id: u64) -> Result<(), String> {
    query(&format!("playid {id}"))?;
    Ok(())
}

/// Reorder existing queue entries without clearing, adding or transporting.
/// MPD preserves the current song and playback state; random mode is untouched.
pub fn shuffle_queue() -> Result<(), String> {
    shuffle_queue_using(|command| query(command).map(|_| ()))
}

fn shuffle_queue_using(mut send: impl FnMut(&str) -> Result<(), String>) -> Result<(), String> {
    send("shuffle")
}

/// Replace the queue with the whole MPD database, randomize it and play.
/// MPD's empty URI addresses the database root; no song metadata is downloaded.
pub fn shuffle_all_songs() -> Result<(), String> {
    let fields = query("stats")?;
    let count = fields
        .iter()
        .find(|(key, _)| key == "songs")
        .and_then(|(_, value)| value.parse::<u64>().ok())
        .ok_or("MPD stats did not return a song count")?;
    if count == 0 {
        return Err("MPD library has no songs; queue left unchanged".into());
    }
    // Only fixed, trusted commands appear in this list. An error stops later commands.
    query_with_timeout(SHUFFLE_COMMANDS, Duration::from_secs(120))?;
    Ok(())
}

/// Append one database song to the queue and start that exact queue ID.
pub fn play_song(file: &str) -> Result<(), String> {
    let fields = query(&format!("addid {}", quote(file)?))?;
    let id = fields
        .iter()
        .find(|(key, _)| key == "Id")
        .and_then(|(_, value)| value.parse::<u64>().ok())
        .ok_or("MPD addid did not return a song ID")?;
    query(&format!("playid {id}"))?;
    Ok(())
}

/// A database-backed source, independent of browser row indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueSource {
    Song(String),
    Album(String, Option<String>),
    Playlist(String),
    Artist(String),
    Folder(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueAction {
    PlayNow,
    Shuffle,
    PlayNext,
    Append,
}

fn source_query(source: &QueueSource) -> Result<Option<String>, String> {
    Ok(Some(match source {
        QueueSource::Song(_) => return Ok(None),
        QueueSource::Album(album, artist) => {
            let mut command = format!("find album {}", quote(album)?);
            if let Some(artist) = artist {
                command.push_str(&format!(" artist {}", quote(artist)?));
            }
            command
        }
        QueueSource::Playlist(name) => format!("listplaylistinfo {}", quote(name)?),
        QueueSource::Artist(name) => format!("find artist {}", quote(name)?),
        // listall recursively lists MPD-indexed files, not the local filesystem.
        QueueSource::Folder(path) => format!("listall {}", quote(path)?),
    }))
}

fn source_files(source: &QueueSource) -> Result<Vec<String>, String> {
    if let QueueSource::Song(file) = source {
        return Ok(vec![file.clone()]);
    }
    let command = source_query(source)?.ok_or("Missing source query")?;
    Ok(ordered_source_files(source, query(&command)?))
}

fn ordered_source_files(source: &QueueSource, fields: Vec<(String, String)>) -> Vec<String> {
    // Saved playlists retain explicit order and duplicates; folders retain MPD's
    // database traversal order. Album/artist playback uses numeric disc/track tags.
    if !matches!(source, QueueSource::Album(_, _) | QueueSource::Artist(_)) {
        return fields
            .into_iter()
            .filter_map(|(k, v)| (k == "file").then_some(v))
            .collect();
    }
    let mut rows: Vec<(String, String, u32, u32)> = Vec::new();
    for (key, value) in fields {
        if key == "file" {
            rows.push((value, String::new(), 1, u32::MAX));
        } else if let Some(row) = rows.last_mut() {
            match key.as_str() {
                "Album" => row.1 = value.to_lowercase(),
                "Disc" => {
                    row.2 = value
                        .split('/')
                        .next()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1)
                }
                "Track" => {
                    row.3 = value
                        .split('/')
                        .next()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(u32::MAX)
                }
                _ => {}
            }
        }
    }
    rows.sort_by(|a, b| (&a.1, a.2, a.3, &a.0).cmp(&(&b.1, b.2, b.3, &b.0)));
    rows.into_iter().map(|row| row.0).collect()
}

fn replacement_commands(files: &[String]) -> Result<String, String> {
    if files.is_empty() {
        return Err("Empty collection; queue unchanged".into());
    }
    // Validate all input before sending clear. MPD stops this command list on an
    // ACK: play is last, so a partially loaded collection is not auto-started.
    let mut commands = vec![
        "command_list_ok_begin".to_string(),
        "clear".into(),
        "random 0".into(),
        "single 0".into(),
    ];
    for file in files {
        commands.push(format!("addid {}", quote(file)?));
    }
    commands.extend(["play 0".into(), "command_list_end".into()]);
    Ok(commands.join("\n"))
}

fn replace_collection(files: &[String]) -> Result<(), String> {
    let command = replacement_commands(files)?;
    query_with_timeout(&command, Duration::from_secs(120)).map_err(|e| {
        format!(
            "Collection replacement failed; queue may be partially replaced (not rolled back): {e}"
        )
    })?;
    Ok(())
}

fn shuffle_files(files: &mut [String], rng: &mut impl rand::Rng) {
    use rand::seq::SliceRandom;
    files.shuffle(rng);
}

fn addition_commands(files: &[String], next: bool) -> Result<String, String> {
    // MPD 0.23+: +0 is immediately after the current item, evaluated by MPD,
    // not a stale UI position. Reverse insertion preserves album/playlist order.
    let mut commands = vec!["command_list_ok_begin".to_string()];
    let ordered: Vec<_> = if next {
        files.iter().rev().collect()
    } else {
        files.iter().collect()
    };
    for file in ordered {
        commands.push(format!(
            "addid {}{}",
            quote(file)?,
            if next { " +0" } else { "" }
        ));
    }
    commands.push("command_list_end".into());
    Ok(commands.join("\n"))
}

pub fn queue_source(source: &QueueSource, action: QueueAction) -> Result<(), String> {
    // Preserve the existing single-song append-and-play transport implementation.
    if let (QueueSource::Song(file), QueueAction::PlayNow) = (source, action) {
        return play_song(file);
    }
    let mut files = source_files(source)?;
    if files.is_empty() {
        return Err("No songs in this source; queue unchanged".into());
    }
    if action == QueueAction::Shuffle {
        // Materialize one randomized collection order, then play it sequentially.
        shuffle_files(&mut files, &mut rand::rng());
    }
    if !matches!(source, QueueSource::Song(_))
        && matches!(action, QueueAction::PlayNow | QueueAction::Shuffle)
    {
        return replace_collection(&files);
    }
    // With no current item, Play Next appends without starting playback.
    let next = action == QueueAction::PlayNext && read_status()?.queue_position.is_some();
    let fields = query_with_timeout(&addition_commands(&files, next)?, Duration::from_secs(120))?;
    if matches!(action, QueueAction::PlayNow | QueueAction::Shuffle) {
        let id = fields
            .iter()
            .find(|(key, _)| key == "Id")
            .and_then(|(_, value)| value.parse::<u64>().ok())
            .ok_or("MPD did not return the added song ID")?;
        play_queue_id(id)?;
    }
    Ok(())
}

pub fn remove_queue_id(id: u64) -> Result<(), String> {
    query(&format!("deleteid {id}"))?;
    Ok(())
}

pub fn clear_queue() -> Result<(), String> {
    clear_queue_using(|| Ok(read_status()?.song_id), query)
}

fn clear_queue_using(
    mut current: impl FnMut() -> Result<Option<u64>, String>,
    mut request: impl FnMut(&str) -> Result<Vec<(String, String)>, String>,
) -> Result<(), String> {
    let original = current()?;
    let ids: Vec<u64> = request("playlistinfo")?
        .into_iter()
        .filter_map(|(key, value)| (key == "Id").then(|| value.parse().ok()).flatten())
        .collect();
    // A single entry is always left alone, even when stopped.
    if ids.len() <= 1 {
        return Ok(());
    }
    for id in removable_ids(&ids, original) {
        // MPD has no conditional delete transaction. Recheck before each delete;
        // never use positions that shift as entries are removed.
        if current()? != original {
            return Err("Current song changed; stopped clearing queue safely".into());
        }
        request(&format!("deleteid {id}"))?;
    }
    Ok(())
}

/// Counts for paginated server queries: do not probe past the queue's end,
/// because playlistinfo/listplaylistinfo may reject an out-of-range start.
pub fn queue_length() -> Result<usize, String> {
    count_field(query("status")?, "playlistlength")
}
pub fn source_length(source: &QueueSource) -> Result<usize, String> {
    if let QueueSource::Playlist(name) = source {
        return Ok(query(&format!("listplaylist {}", quote(name)?))?
            .iter()
            .filter(|(key, _)| key == "file")
            .count());
    }
    if matches!(source, QueueSource::Artist(_) | QueueSource::Album(_, _)) {
        let find = source_query(source)?.ok_or("Missing source query")?;
        return count_field(
            query(&format!(
                "count {}",
                find.strip_prefix("find ").ok_or("Invalid count source")?
            ))?,
            "songs",
        );
    }
    Ok(source_files(source)?.len())
}
pub fn library_length() -> Result<usize, String> {
    count_field(query("stats")?, "songs")
}
fn count_field(fields: Vec<(String, String)>, name: &str) -> Result<usize, String> {
    fields
        .into_iter()
        .find(|(key, _)| key == name)
        .and_then(|(_, value)| value.parse().ok())
        .ok_or_else(|| format!("Missing {name} count"))
}

fn removable_ids(ids: &[u64], current: Option<u64>) -> Vec<u64> {
    ids.iter()
        .copied()
        .filter(|id| Some(*id) != current)
        .collect()
}

/// MPD binary artwork protocol, separate from line-only library responses.
/// A worker calls this only when the current file changes.
pub fn artwork(file: &str, embedded: bool) -> Result<Vec<u8>, String> {
    let (host, port) = endpoint()?;
    let mut reader = None;
    for address in (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
    {
        if let Ok(stream) = TcpStream::connect_timeout(&address, TIMEOUT) {
            stream
                .set_read_timeout(Some(TIMEOUT))
                .map_err(|e| e.to_string())?;
            stream
                .set_write_timeout(Some(TIMEOUT))
                .map_err(|e| e.to_string())?;
            reader = Some(BufReader::new(stream));
            break;
        }
    }
    let mut reader = reader.ok_or("Cannot connect for artwork")?;
    let mut greeting = String::new();
    reader.read_line(&mut greeting).map_err(|e| e.to_string())?;
    if !greeting.starts_with("OK MPD ") {
        return Err("Not MPD".into());
    }
    let command = if embedded { "readpicture" } else { "albumart" };
    let file = quote(file)?;
    let mut data = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut expected = None;
    loop {
        if std::time::Instant::now() > deadline {
            return Err("Artwork timed out".into());
        }
        writeln!(reader.get_mut(), "{command} {file} {}", data.len()).map_err(|e| e.to_string())?;
        reader.get_mut().flush().map_err(|e| e.to_string())?;
        let (size, chunk) = artwork_chunk(&mut reader)?;
        if expected.is_some_and(|old| old != size) {
            return Err("Artwork changed during transfer".into());
        }
        expected = Some(size);
        if chunk.is_empty() || data.len() + chunk.len() > size {
            return Err("Invalid artwork chunk".into());
        }
        data.extend(chunk);
        if data.len() == size {
            return Ok(data);
        }
    }
}

fn artwork_chunk(reader: &mut impl BufRead) -> Result<(usize, Vec<u8>), String> {
    const MAX_ART: usize = 8 * 1024 * 1024;
    let mut size = None;
    let mut data = None;
    for _ in 0..16 {
        let mut line = String::new();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("Truncated artwork reply".into());
        }
        let line = line.trim_end();
        if line.starts_with("ACK ") {
            return Err(line.into());
        }
        if line == "OK" {
            return Ok((
                size.ok_or("Missing artwork size")?,
                data.ok_or("Missing artwork bytes")?,
            ));
        }
        if let Some(value) = line.strip_prefix("size: ") {
            let value: usize = value.parse().map_err(|_| "Invalid artwork size")?;
            if value == 0 || value > MAX_ART {
                return Err("Artwork too large or empty".into());
            }
            size = Some(value);
        } else if let Some(value) = line.strip_prefix("binary: ") {
            let count: usize = value.parse().map_err(|_| "Invalid binary length")?;
            if count > size.unwrap_or(0) || data.is_some() {
                return Err("Invalid binary chunk".into());
            }
            let mut bytes = vec![0; count];
            reader.read_exact(&mut bytes).map_err(|e| e.to_string())?;
            let mut newline = [0];
            reader.read_exact(&mut newline).map_err(|e| e.to_string())?;
            if newline != *b"\n" {
                return Err("Invalid binary delimiter".into());
            }
            data = Some(bytes);
        }
    }
    Err("Artwork headers too long".into())
}

#[derive(Debug, Clone, PartialEq)]
pub enum DirectoryEntry {
    Folder(String),
    Song(Song),
}

fn directory_entries(fields: Vec<(String, String)>) -> Vec<DirectoryEntry> {
    let mut result = Vec::new();
    let mut song_fields = Vec::new();
    for (key, value) in fields {
        if matches!(key.as_str(), "file" | "directory" | "playlist") {
            for song in songs_from_fields(std::mem::take(&mut song_fields)) {
                result.push(DirectoryEntry::Song(song));
            }
        }
        if key == "directory" {
            result.push(DirectoryEntry::Folder(value));
        } else {
            song_fields.push((key, value));
        }
    }
    result.extend(
        songs_from_fields(song_fields)
            .into_iter()
            .map(DirectoryEntry::Song),
    );
    // lsinfo includes stored playlist records: intentionally ignore those here.
    result.sort_by_key(|entry| match entry {
        DirectoryEntry::Folder(path) => (0, path.to_lowercase()),
        DirectoryEntry::Song(song) => (1, song.title.to_lowercase()),
    });
    result
}

pub fn browse_directory(path: &str) -> Result<Vec<DirectoryEntry>, String> {
    Ok(directory_entries(query(&format!(
        "lsinfo {}",
        quote(path)?
    ))?))
}

#[cfg(test)]
mod tests {
    #[test]
    fn queue_shuffle_only_reorders_and_propagates_failure() {
        let mut commands = Vec::new();
        shuffle_queue_using(|command| {
            commands.push(command.to_owned());
            Ok(())
        })
        .unwrap();
        assert_eq!(commands, ["shuffle"]);
        let mut attempts = 0;
        let result = shuffle_queue_using(|command| {
            attempts += 1;
            assert_eq!(command, "shuffle");
            Err("disconnected".into())
        });
        assert_eq!(result, Err("disconnected".into()));
        assert_eq!(attempts, 1, "never replay a mutation automatically");
    }

    #[test]
    fn snapshot_batches_two_commands_on_one_connection() {
        // Duplex fixture: no ports or process-global MPD environment changes.
        struct Wire {
            reply: std::io::Cursor<&'static [u8]>,
            sent: Vec<u8>,
        }
        impl std::io::Read for Wire {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                self.reply.read(bytes)
            }
        }
        impl Write for Wire {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.sent.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let wire = Wire {
            reply: std::io::Cursor::new(b"songid: 7\nelapsed: 12\nduration: 99\nstate: pause\nlist_OK\nfile: fixture.flac\nId: 7\nTitle: Fixture\nduration: 100\nlist_OK\nOK\n"),
            sent: Vec::new(),
        };
        let mut reader = BufReader::new(wire);
        let (state, song) = snapshot_on_connection(&mut reader).unwrap();
        assert_eq!(
            reader.get_ref().sent,
            b"command_list_ok_begin\nstatus\ncurrentsong\ncommand_list_end\n"
        );
        assert_eq!(state.song_id, song.id);
        assert_eq!(state.elapsed, 12.0);
        assert_eq!(state.duration, 99.0);
        assert_eq!(song.duration, 100.0);
        assert!(state.paused);
        assert_eq!(
            reader.get_ref().reply.position(),
            reader.get_ref().reply.get_ref().len() as u64
        );
    }

    #[test]
    fn snapshot_retries_mismatches_once_without_mixing_metadata() {
        let pair = |id, song_id| {
            let state = PlayerState {
                song_id: Some(id),
                elapsed: id as f64,
                ..Default::default()
            };
            let song = parse_song_response(&mut std::io::Cursor::new(format!(
                "Id: {song_id}\nfile: {song_id}.flac\nduration: 100\nOK\n"
            )))
            .unwrap();
            Ok((state, song))
        };
        let mut calls = 0;
        let state = read_player_state_using(|| {
            calls += 1;
            pair(calls, 2)
        })
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(state.file, "2.flac");
        assert_eq!(state.elapsed, 2.0);
        assert_eq!(state.duration, 100.0);
        calls = 0;
        assert!(
            read_player_state_using(|| {
                calls += 1;
                pair(calls, calls + 1)
            })
            .is_err()
        );
        assert_eq!(calls, 2);
    }

    #[test]
    fn snapshot_rejects_missing_boundaries_errors_and_truncation() {
        use std::io::Cursor;
        for reply in ["songid: 1\nOK\n", "ACK [5@0] {} error\n", "songid: 1\n"] {
            assert!(parse_status_response_until(&mut Cursor::new(reply), "list_OK").is_err());
        }
        for reply in ["Id: 1\nOK\n", "ACK [5@1] {} error\n", "Id: 1\n"] {
            assert!(parse_song_response_until(&mut Cursor::new(reply), "list_OK").is_err());
        }
        assert!(response(&mut Cursor::new("")).is_err());
        assert!(response(&mut Cursor::new("ACK [5@1] {} error\n")).is_err());
    }

    use super::*;
    use std::io::Cursor;

    #[test]
    fn collection_replacement_drops_old_queue_and_plays_in_order() {
        let files: Vec<_> = (1..=12).map(|n| format!("album/{n:02}.flac")).collect();
        let commands = replacement_commands(&files).unwrap();
        let lines: Vec<_> = commands.lines().collect();
        assert_eq!(
            &lines[..4],
            ["command_list_ok_begin", "clear", "random 0", "single 0"]
        );
        assert_eq!(&lines[lines.len() - 2..], ["play 0", "command_list_end"]);
        let mut queue: Vec<String> = (0..150).map(|n| format!("old-{n}")).collect();
        let mut random = true;
        let mut position = None;
        for line in lines {
            if line == "clear" {
                queue.clear();
            } else if line == "random 0" {
                random = false;
            } else if let Some(file) = line.strip_prefix("addid \"") {
                queue.push(file.trim_end_matches('"').to_string());
            } else if line == "play 0" {
                position = Some(0);
            }
        }
        assert_eq!(queue, files);
        assert_eq!(queue.len(), 12);
        assert_eq!(position, Some(0));
        assert!(!random);
        assert_eq!(queue[1], "album/02.flac");
        // Invalid/empty sources are rejected before clear can be sent.
        assert!(replacement_commands(&[]).is_err());
        assert!(replacement_commands(&["valid.flac".into(), "bad\nclear".into()]).is_err());
        // Partial MPD command-list failures are errors, never success/implicit rollback.
        assert!(
            read_fields(&mut Cursor::new(
                b"list_OK\nlist_OK\nlist_OK\nId: 99\nlist_OK\nACK [50@4] {addid} Missing\n"
            ))
            .is_err()
        );
    }

    #[test]
    fn collection_order_uses_numeric_disc_track_and_preserves_playlist_duplicates() {
        let fields = read_fields(&mut Cursor::new(b"file: ten.flac\nAlbum: Album\nDisc: 1/2\nTrack: 10/12\nfile: second-disc.flac\nAlbum: Album\nDisc: 2\nTrack: 1\nfile: two.flac\nAlbum: Album\nDisc: 1\nTrack: 2\nOK\n")).unwrap();
        for source in [
            QueueSource::Album("Album".into(), None),
            QueueSource::Artist("Artist".into()),
        ] {
            assert_eq!(
                ordered_source_files(&source, fields.clone()),
                ["two.flac", "ten.flac", "second-disc.flac"]
            );
        }
        let playlist = vec![
            ("file".into(), "b.flac".into()),
            ("file".into(), "a.flac".into()),
            ("file".into(), "b.flac".into()),
        ];
        assert_eq!(
            ordered_source_files(&QueueSource::Playlist("saved".into()), playlist),
            ["b.flac", "a.flac", "b.flac"]
        );
    }

    #[test]
    fn saved_playlist_mutations_are_quoted_and_do_not_touch_active_queue() {
        assert_eq!(
            playlist_add_command("Road \"Mix\"", "a song.flac").unwrap(),
            "playlistadd \"Road \\\"Mix\\\"\" \"a song.flac\""
        );
        assert!(playlist_add_command("x\nclear", "a").is_err());
        assert!(playlist_add_command("x", "a\nplay").is_err());
        let command = playlist_add_command("saved", "a.flac").unwrap();
        assert_eq!(command.lines().count(), 1);
        assert!(!command.starts_with("addid") && !command.starts_with("clear"));
    }

    #[test]
    fn playback_settings_parse_real_status_and_generate_native_commands() {
        let state = parse_status_response(&mut Cursor::new(
            b"volume: 73\nxfade: 3\nrepeat: 1\nrandom: 1\nOK\n",
        ))
        .unwrap();
        assert_eq!(state.volume, Some(73));
        assert_eq!(state.crossfade, 3.0);
        assert!(state.repeat && state.random);
        assert_eq!(PlaybackSetting::Volume(73).command().unwrap(), "setvol 73");
        assert!(PlaybackSetting::Volume(101).command().is_err());
        assert_eq!(
            PlaybackSetting::Crossfade(0).command().unwrap(),
            "crossfade 0"
        );
        assert_eq!(
            PlaybackSetting::Repeat(false).command().unwrap(),
            "repeat 0"
        );
        assert_eq!(PlaybackSetting::Random(true).command().unwrap(), "random 1");
    }

    #[test]
    fn previous_restarts_only_above_three_seconds() {
        let mut state = PlayerState {
            song_id: Some(42),
            playing: true,
            elapsed: 3.0,
            ..Default::default()
        };
        assert_eq!(previous_command(&state), "previous");
        state.elapsed = 3.001;
        assert_eq!(previous_command(&state), "seekid 42 0");
        state.playing = false;
        state.paused = true;
        assert_eq!(previous_command(&state), "seekid 42 0");
        state.elapsed = 0.0;
        assert_eq!(previous_command(&state), "previous");
    }

    #[test]
    fn seek_is_id_bound_clamped_and_preserves_pause() {
        let mut state = PlayerState {
            song_id: Some(42),
            duration: 126.0,
            paused: true,
            ..Default::default()
        };
        assert_eq!(seek_command(42, 83.0, &state).unwrap(), "seekid 42 83.000");
        assert_eq!(seek_command(42, -4.0, &state).unwrap(), "seekid 42 0.000");
        assert_eq!(
            seek_command(42, 500.0, &state).unwrap(),
            "seekid 42 125.999"
        );
        assert!(seek_command(99, 20.0, &state).is_err());
        assert!(seek_command(42, f64::NAN, &state).is_err());
        state.duration = 0.0;
        assert!(seek_command(42, 0.0, &state).is_err());
        state.duration = 126.0;
        state.paused = false;
        assert!(seek_command(42, 20.0, &state).is_err());
        state.playing = true;
        assert!(seek_command(42, 20.0, &state).is_ok());
    }

    #[test]
    fn queue_indicator_fields_come_from_status() {
        let state = parse_status_response(&mut Cursor::new(
            b"song: 0\nplaylistlength: 14\nsongid: 42\nOK\n",
        ))
        .unwrap();
        assert_eq!(state.queue_position, Some(0));
        assert_eq!(state.queue_length, Some(14));
        let empty =
            parse_status_response(&mut Cursor::new(b"playlistlength: 0\nstate: stop\nOK\n"))
                .unwrap();
        assert_eq!(empty.queue_position, None);
        assert_eq!(empty.queue_length, Some(0));
    }

    #[test]
    fn clear_keeps_current_id_without_transport_commands() {
        for keep in [Some(10), Some(20), Some(30), None] {
            let mut commands = Vec::new();
            clear_queue_using(
                || Ok(keep),
                |command| {
                    commands.push(command.to_string());
                    if command == "playlistinfo" {
                        Ok([10, 20, 30]
                            .iter()
                            .map(|id| ("Id".into(), id.to_string()))
                            .collect())
                    } else {
                        Ok(vec![])
                    }
                },
            )
            .unwrap();
            let expected: Vec<_> = [10, 20, 30]
                .into_iter()
                .filter(|id| Some(*id) != keep)
                .map(|id| format!("deleteid {id}"))
                .collect();
            assert_eq!(&commands[1..], expected);
        }
        let mut calls = 0;
        clear_queue_using(
            || Ok(Some(20)),
            |_| {
                calls += 1;
                Ok(vec![("Id".into(), "20".into())])
            },
        )
        .unwrap();
        assert_eq!(calls, 1);
    }

    #[test]
    fn clear_aborts_on_track_change_before_deleting() {
        let mut states = [Some(10), Some(20)].into_iter();
        let mut commands = Vec::new();
        assert!(
            clear_queue_using(
                || Ok(states.next().unwrap()),
                |command| {
                    commands.push(command.to_string());
                    Ok(vec![("Id".into(), "10".into()), ("Id".into(), "20".into())])
                }
            )
            .is_err()
        );
        assert_eq!(commands, ["playlistinfo"]);
    }

    #[test]
    fn binary_artwork_reads_exact_bytes_not_lines() {
        let (size, data) = artwork_chunk(&mut Cursor::new(
            b"size: 5\ntype: image/png\nbinary: 5\nA\n\0BC\nOK\n",
        ))
        .unwrap();
        assert_eq!(size, 5);
        assert_eq!(data, b"A\n\0BC");
        for bytes in [
            b"size: 999999999\nOK\n".as_slice(),
            b"size: 1\nbinary: 2\n",
            b"size: 3\nbinary: 3\na",
            b"ACK [50@0] {albumart} No art\n",
            b"size: 1\nbinary: 1\naXOK\n",
        ] {
            assert!(artwork_chunk(&mut Cursor::new(bytes)).is_err());
        }
    }

    #[test]
    fn state_tracks_file_and_stable_id() {
        let state = parse_status_response(&mut Cursor::new(
            b"state: play\nsongid: 42\nelapsed: 0.5\nOK\n",
        ))
        .unwrap();
        let song =
            parse_song_response(&mut Cursor::new(b"file: nested/song.flac\nId: 42\nOK\n")).unwrap();
        assert_eq!(state.song_id, song.id);
        assert_eq!(song.file, "nested/song.flac");
        assert_eq!(state.elapsed, 0.5);
    }

    #[test]
    fn source_queries_are_scoped_and_quoted() {
        assert_eq!(
            source_query(&QueueSource::Artist("Jane \"Live\"".into()))
                .unwrap()
                .unwrap(),
            "find artist \"Jane \\\"Live\\\"\""
        );
        assert_eq!(
            source_query(&QueueSource::Folder("Music/Jazz".into()))
                .unwrap()
                .unwrap(),
            "listall \"Music/Jazz\""
        );
        assert_eq!(
            source_query(&QueueSource::Playlist("Favourites".into()))
                .unwrap()
                .unwrap(),
            "listplaylistinfo \"Favourites\""
        );
        assert_eq!(
            source_query(&QueueSource::Album("Live".into(), Some("Jane".into())))
                .unwrap()
                .unwrap(),
            "find album \"Live\" artist \"Jane\""
        );
        assert!(source_query(&QueueSource::Folder("bad\nclear".into())).is_err());
        assert!(source_query(&QueueSource::Artist("bad\nclear".into())).is_err());
    }

    #[test]
    fn scoped_shuffle_is_a_permutation_and_only_appends() {
        use rand::SeedableRng;
        let original: Vec<String> = (0..20).map(|i| format!("{i:02}.flac")).collect();
        let mut files = original.clone();
        shuffle_files(&mut files, &mut rand::rngs::StdRng::seed_from_u64(42));
        assert_ne!(files, original);
        let mut sorted = files.clone();
        sorted.sort();
        assert_eq!(sorted, original);
        let commands = addition_commands(&files, false).unwrap();
        assert_eq!(
            commands.lines().nth(1).unwrap(),
            format!("addid {}", quote(&files[0]).unwrap())
        );
        assert!(
            commands
                .lines()
                .skip(1)
                .take(files.len())
                .all(|line| line.starts_with("addid ") && !line.ends_with(" +0"))
        );
        assert!(
            !commands
                .lines()
                .any(|line| line == "clear" || line == "shuffle" || line.starts_with("random "))
        );
        let mut empty = vec![];
        shuffle_files(&mut empty, &mut rand::rngs::StdRng::seed_from_u64(1));
        assert!(empty.is_empty());
        let mut single = vec!["one.flac".into()];
        shuffle_files(&mut single, &mut rand::rngs::StdRng::seed_from_u64(1));
        assert_eq!(single, ["one.flac"]);
    }

    #[test]
    fn queue_additions_preserve_order_and_use_server_relative_positions() {
        let files = vec![
            "album/01.flac".into(),
            "album/02.flac".into(),
            "album/03.flac".into(),
        ];
        assert_eq!(
            addition_commands(&files, false).unwrap(),
            "command_list_ok_begin\naddid \"album/01.flac\"\naddid \"album/02.flac\"\naddid \"album/03.flac\"\ncommand_list_end"
        );
        assert_eq!(
            addition_commands(&files, true).unwrap(),
            "command_list_ok_begin\naddid \"album/03.flac\" +0\naddid \"album/02.flac\" +0\naddid \"album/01.flac\" +0\ncommand_list_end"
        );
        // Simulate each insertion directly after the current item.
        let mut queue = vec!["current".to_string(), "old-next".to_string()];
        for file in files.iter().rev() {
            queue.insert(1, file.clone());
        }
        assert_eq!(&queue[1..4], files.as_slice());
    }

    #[test]
    fn queue_additions_quote_paths_before_any_mutation() {
        assert!(addition_commands(&["good.mp3".into(), "bad\nclear".into()], false).is_err());
        assert_eq!(
            addition_commands(&["a\"b.flac".into()], true).unwrap(),
            "command_list_ok_begin\naddid \"a\\\"b.flac\" +0\ncommand_list_end"
        );
    }

    #[test]
    fn directory_listing_separates_folders_songs_and_stored_playlists() {
        let fields = read_fields(&mut Cursor::new(
            b"file: root/z.flac\nTitle: Zed\nArtist: Someone\ndirectory: root/Folder\nLast-Modified: yesterday\nplaylist: root/mix.m3u\nLast-Modified: today\nfile: root/a.flac\nduration: 12.5\nOK\n"
        )).unwrap();
        let rows = directory_entries(fields);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], DirectoryEntry::Folder("root/Folder".into()));
        assert!(
            matches!(&rows[1], DirectoryEntry::Song(song) if song.file == "root/a.flac" && song.title == "a.flac" && song.duration == 12.5)
        );
        assert!(
            matches!(&rows[2], DirectoryEntry::Song(song) if song.title == "Zed" && song.artist == "Someone")
        );
        assert!(directory_entries(Vec::new()).is_empty());
    }

    #[test]
    fn command_list_collects_stable_ids_in_addition_order() {
        let fields =
            read_fields(&mut Cursor::new(b"Id: 41\nlist_OK\nId: 99\nlist_OK\nOK\n")).unwrap();
        assert_eq!(
            fields,
            vec![("Id".into(), "41".into()), ("Id".into(), "99".into())]
        );
        assert!(
            read_fields(&mut Cursor::new(
                b"Id: 41\nlist_OK\nACK [50@1] {addid} Missing file\n"
            ))
            .is_err()
        );
    }

    #[test]
    fn parses_queue_ids_positions_and_status_selection() {
        assert_eq!(
            SHUFFLE_COMMANDS.lines().collect::<Vec<_>>(),
            [
                "command_list_ok_begin",
                "clear",
                "add \"\"",
                "shuffle",
                "play 0",
                "command_list_end"
            ]
        );
        let fields = read_fields(&mut Cursor::new(
            b"file: a.mp3\nTitle: First\nPos: 0\nId: 12\nfile: b.mp3\nPos: 1\nId: 99\nOK\n",
        ))
        .unwrap();
        let queue = queue_from_fields(fields);
        assert_eq!(queue.len(), 2);
        assert_eq!((queue[0].id, queue[0].position), (12, 0));
        assert_eq!((queue[1].id, queue[1].position), (99, 1));
        assert_eq!(queue[1].song.title, "b.mp3");
        let status = parse_status_response(&mut Cursor::new(
            b"state: pause\nsong: 1\nplaylist: 42\nOK\n",
        ))
        .unwrap();
        assert_eq!(status.queue_position, Some(1));
        assert_eq!(status.queue_version, Some(42));
        assert_eq!(page_range(6, 7).unwrap(), "6:13");
    }

    #[test]
    fn library_protocol_parsing_and_quoting() {
        assert_eq!(quote("Jane").unwrap(), "\"Jane\"");
        assert_eq!(quote("a\"b").unwrap(), "\"a\\\"b\"");
        assert!(quote("artist\nplay").is_err());
        assert_eq!(window(6, 7).unwrap(), "window 6:13");
        let fields = read_fields(&mut Cursor::new(
            b"file: jazz/one.flac\nArtist: Jane\nTitle: One\nfile: two.mp3\nduration: 2.5\nOK\n",
        ))
        .unwrap();
        let songs = songs_from_fields(fields);
        assert_eq!(songs.len(), 2);
        assert_eq!(songs[0].file, "jazz/one.flac");
        assert_eq!(songs[0].title, "One");
        assert_eq!(songs[0].artist, "Jane");
        assert_eq!(songs[1].title, "two.mp3");
        assert_eq!(songs[1].duration, 2.5);
        assert!(read_fields(&mut Cursor::new(b"ACK [2@0] {find} bad\n")).is_err());
        assert!(read_fields(&mut Cursor::new(b"file: half.mp3\n")).is_err());
    }

    #[test]
    fn maps_only_known_ui_actions() {
        assert_eq!(Transport::from_ui("play-pause"), Some(Transport::PlayPause));
        assert_eq!(Transport::from_ui("play"), Some(Transport::Play));
        assert_eq!(Transport::from_ui("pause"), Some(Transport::Pause));
        assert_eq!(Transport::from_ui("unknown"), None);
    }

    #[test]
    fn command_names_and_responses() {
        assert_eq!(Transport::Previous.command(), "previous");
        assert_eq!(Transport::Next.command(), "next");
        assert_eq!(Transport::PlayPause.command(), "pause");
        assert_eq!(Transport::Play.command(), "play");
        assert_eq!(Transport::Pause.command(), "pause 1");
    }

    #[test]
    fn parse_decimal_time_formats() {
        // Minutes:seconds.centiseconds
        assert!((parse_decimal_time("0:03:45.12").unwrap() - 225.12).abs() < 0.001);
        assert!((parse_decimal_time("0:01:23.45").unwrap() - 83.45).abs() < 0.001);

        // Hours:minutes:seconds.centiseconds
        assert!((parse_decimal_time("1:03:45.12").unwrap() - 3825.12).abs() < 0.001);
        assert!((parse_decimal_time("0:10:30.00").unwrap() - 630.0).abs() < 0.001);

        // Edge cases
        assert_eq!(parse_decimal_time("0:00:00.00").unwrap(), 0.0);
        assert_eq!(parse_decimal_time("0:05:00").unwrap(), 300.0);
    }

    #[test]
    fn test_parse_status_response() {
        // MPD status response format (fields starting with lowercase letters)
        let response = b"elapsed: 0:01:23.45\nduration: 0:03:45.12\nstate: play\nsongname: /music/artist/album/song.mp3\nartist: The Beatles\nalbum: Abbey Road\nOK\n";
        let mut reader = Cursor::new(response);

        let state = parse_status_response(&mut reader).unwrap();

        assert_eq!(state.title, "song.mp3");
        assert_eq!(state.artist, "The Beatles");
        assert_eq!(state.album, "Abbey Road");
        assert!((state.elapsed - 83.45).abs() < 0.01);
        assert!((state.duration - 225.12).abs() < 0.01);
        assert!(state.playing);
    }

    #[test]
    fn test_parse_song_response() {
        let response = b"file: /music/song.mp3\nTitle: Midnight\nArtist: John Doe\nAlbum: Singles Collection\nduration: 0:04:12.33\nOK\n";
        let mut reader = Cursor::new(response);

        let info = parse_song_response(&mut reader).unwrap();

        assert_eq!(info.title, "Midnight");
        assert_eq!(info.artist, "John Doe");
        assert_eq!(info.album, "Singles Collection");
        assert!((info.duration - 252.33).abs() < 0.001);
    }

    #[test]
    fn player_state_struct_fields() {
        let state = PlayerState {
            file: "test.flac".into(),
            song_id: Some(12),
            title: "Test Song".to_string(),
            artist: "Test Artist".to_string(),
            album: "Test Album".to_string(),
            elapsed: 123.45,
            duration: 300.0,
            playing: true,
            paused: false,
            volume: Some(80),
            crossfade: 0.0,
            repeat: false,
            random: false,
            queue_position: Some(2),
            queue_length: Some(14),
            queue_version: Some(5),
        };

        assert_eq!(state.title, "Test Song");
        assert_eq!(state.artist, "Test Artist");
        assert_eq!(state.album, "Test Album");
        assert!((state.elapsed - 123.45).abs() < 0.001);
        assert!((state.duration - 300.0).abs() < 0.001);
        assert!(state.playing);
        assert_eq!(state.volume, Some(80));
    }

    #[test]
    fn song_info_struct_fields() {
        let info = SongInfo {
            file: "test.flac".into(),
            id: Some(12),
            title: "Test Track".to_string(),
            artist: "Test Artist".to_string(),
            album: "Test Album".to_string(),
            duration: 180.5,
        };

        assert_eq!(info.title, "Test Track");
        assert_eq!(info.artist, "Test Artist");
        assert_eq!(info.album, "Test Album");
        assert!((info.duration - 180.5).abs() < 0.001);
    }

    #[test]
    fn parses_realistic_status_and_empty_song() {
        let mut status =
            Cursor::new(b"volume: 80\nstate: pause\nelapsed: 83.45\nduration: 225.12\nOK\n");
        let state = parse_status_response(&mut status).unwrap();
        assert!(!state.playing);
        assert!(state.paused);
        assert_eq!(state.title, ""); // Metadata belongs to currentsong, not status.
        assert_eq!(state.elapsed, 83.45);
        assert_eq!(state.duration, 225.12);
        assert_eq!(state.volume, Some(80));
        let mut song = Cursor::new(b"OK\n");
        assert_eq!(parse_song_response(&mut song).unwrap().title, "");
    }

    #[test]
    fn rejects_mpd_errors_and_truncated_replies() {
        assert!(parse_status_response(&mut Cursor::new(b"ACK [5@0] {status} error\n")).is_err());
        assert!(parse_song_response(&mut Cursor::new(b"file: song.mp3\n")).is_err());
        assert!(parse_decimal_time("not a number").is_err());
        let mut no_mixer = Cursor::new(b"volume: -1\nstate: stop\nOK\n");
        let state = parse_status_response(&mut no_mixer).unwrap();
        assert_eq!(state.volume, None);
        assert!(!state.playing);
        assert!(!state.paused);
    }
}
