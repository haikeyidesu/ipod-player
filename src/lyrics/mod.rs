//! Lyrics are independent of the MPD protocol. Only the current track is fetched.
mod cache;
mod client;
mod parser;

use crate::AppWindow;
use parser::{LyricLine, active_line};
use serde::{Deserialize, Serialize};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub file: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
}

impl Track {
    pub fn new(file: &str, title: &str, artist: &str, album: &str, duration: f64) -> Self {
        Self {
            file: file.into(),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            duration_ms: if duration.is_finite() {
                (duration.max(0.0) * 1000.0).round() as u64
            } else {
                0
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Lyrics {
    Synced(Vec<LyricLine>),
    Plain(String),
    Instrumental,
    Missing,
}

impl Lyrics {
    fn from_text(synced: Option<&str>, plain: Option<&str>, instrumental: bool) -> Self {
        if instrumental {
            return Self::Instrumental;
        }
        let lines = parser::parse(synced.unwrap_or_default());
        if lines.iter().any(|line| !line.text.is_empty()) {
            return Self::Synced(lines);
        }
        match plain.map(str::trim).filter(|s| !s.is_empty()) {
            Some(text) => Self::Plain(text.to_owned()),
            None => Self::Missing,
        }
    }
}

#[derive(Default)]
struct Presentation {
    track: Option<Track>,
    song_id: String,
    generation: u64,
    lines: Vec<LyricLine>,
}

impl Presentation {
    fn accepts(&self, generation: u64) -> bool {
        self.generation == generation
    }
    fn show_elapsed(&self, app: &AppWindow) {
        let index = active_line(&self.lines, app.get_player_elapsed() as f64)
            .map(|n| n as i32)
            .unwrap_or(-1);
        if app.get_lyrics_active() != index {
            app.set_lyrics_active(index);
            let preceding = self
                .lines
                .iter()
                .take(index.max(0) as usize)
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            app.set_lyrics_prefix(
                if preceding.is_empty() {
                    String::new()
                } else {
                    format!("{preceding}\n\n")
                }
                .into(),
            );
        }
    }
    fn apply(&mut self, app: &AppWindow, generation: u64, lyrics: Lyrics) {
        if !self.accepts(generation) {
            return;
        }
        self.lines.clear();
        app.set_lyrics_active(-1);
        app.set_lyrics_lines(ModelRc::default());
        app.set_lyrics_plain("".into());
        app.set_lyrics_transcript("".into());
        app.set_lyrics_prefix("".into());
        app.set_lyrics_message("".into());
        match lyrics {
            Lyrics::Synced(lines) => {
                app.set_lyrics_transcript(
                    lines
                        .iter()
                        .map(|l| l.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n\n")
                        .into(),
                );
                app.set_lyrics_lines(ModelRc::new(VecModel::from(
                    lines
                        .iter()
                        .map(|l| SharedString::from(&l.text))
                        .collect::<Vec<_>>(),
                )));
                self.lines = lines;
            }
            Lyrics::Plain(text) => app.set_lyrics_plain(text.into()),
            Lyrics::Instrumental => app.set_lyrics_message("Instrumental".into()),
            Lyrics::Missing => app.set_lyrics_message("No lyrics available".into()),
        }
        self.show_elapsed(app);
    }
}

pub struct LyricsService {
    presentation: Rc<RefCell<Presentation>>,
    generation: Arc<AtomicU64>,
    requests: mpsc::Sender<(u64, Track)>,
}

impl LyricsService {
    pub fn install(app: &AppWindow) -> Self {
        let presentation = Rc::new(RefCell::new(Presentation::default()));
        let generation = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel::<(u64, Track)>();
        let weak = app.as_weak();
        // UI-owned presentation is accessed only in the event loop via this callback.
        let state = presentation.clone();
        app.on_lyrics_delivered(move |generation, payload| {
            if let Some(app) = weak.upgrade()
                && let (Ok(generation), Ok(lyrics)) = (
                    generation.parse::<u64>(),
                    serde_json::from_str::<Lyrics>(&payload),
                )
            {
                state.borrow_mut().apply(&app, generation, lyrics);
            }
        });
        let weak = app.as_weak();
        let current = generation.clone();
        std::thread::spawn(move || {
            let cache = cache::Cache::application();
            let mut client = client::Client::new();
            while let Ok((mut ticket, mut track)) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    (ticket, track) = newer;
                }
                let valid = || current.load(Ordering::Acquire) == ticket;
                if !valid() {
                    continue;
                }
                let cached = cache.as_ref().and_then(|cache| cache.load(&track));
                let lyrics = if let Some(lyrics) = cached {
                    lyrics
                } else {
                    match client
                        .as_mut()
                        .map_err(|e| e.to_string())
                        .and_then(|c| c.fetch(&track, &valid))
                    {
                        Ok(lyrics) => {
                            if let Some(cache) = &cache {
                                if let Err(error) = cache.save(&track, &lyrics) {
                                    eprintln!("Lyrics cache write failed: {error}");
                                }
                            } else {
                                eprintln!(
                                    "Lyrics cache unavailable: no application-support directory"
                                );
                            }
                            lyrics
                        }
                        Err(error) => {
                            if valid() {
                                eprintln!("Lyrics: {error}");
                            }
                            Lyrics::Missing
                        }
                    }
                };
                if !valid() {
                    continue;
                }
                let weak = weak.clone();
                // All file/network/parsing/serialization work stays on this worker.
                let payload = serde_json::to_string(&lyrics).unwrap_or_default();
                if slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        app.invoke_lyrics_delivered(ticket.to_string().into(), payload.into());
                    }
                })
                .is_err()
                {
                    break;
                }
            }
        });
        Self {
            presentation,
            generation,
            requests: tx,
        }
    }

    /// Call after setting elapsed. Stable tracks only update active-line selection;
    /// they never enqueue another HTTP/cache request, even while disconnected.
    pub fn update(&self, app: &AppWindow, track: Track, song_id: &str) {
        let mut state = self.presentation.borrow_mut();
        if state.track.as_ref() != Some(&track) || state.song_id != song_id {
            state.generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
            state.track = Some(track.clone());
            state.song_id = song_id.to_owned();
            state.lines.clear();
            app.set_lyrics_lines(ModelRc::default());
            app.set_lyrics_plain("".into());
            app.set_lyrics_transcript("".into());
            app.set_lyrics_prefix("".into());
            app.set_lyrics_manual(false);
            app.set_lyrics_active(-1);
            app.set_lyrics_scroll(0);
            app.set_lyrics_message(
                if track.file.is_empty() {
                    "No lyrics available"
                } else {
                    "Loading lyrics…"
                }
                .into(),
            );
            if !track.file.is_empty() {
                let _ = self.requests.send((state.generation, track));
            }
        }
        state.show_elapsed(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_plain_and_synced_preference() {
        assert_eq!(Lyrics::from_text(None, None, false), Lyrics::Missing);
        assert_eq!(
            Lyrics::from_text(Some("bad"), Some("Words"), false),
            Lyrics::Plain("Words".into())
        );
        assert!(matches!(
            Lyrics::from_text(Some("[00:01]Words"), Some("Words"), false),
            Lyrics::Synced(_)
        ));
        assert_eq!(Lyrics::from_text(None, None, true), Lyrics::Instrumental);
    }
    #[test]
    fn delayed_completion_cannot_replace_new_track_and_seeks_reselect() {
        use slint::platform::{
            Platform, PlatformError, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let app = AppWindow::new().unwrap();
        let mut state = Presentation {
            generation: 2,
            ..Default::default()
        };
        state.apply(&app, 2, Lyrics::Plain("New song".into()));
        state.apply(&app, 1, Lyrics::Plain("Late old song".into()));
        assert_eq!(app.get_lyrics_plain(), "New song");
        state.apply(
            &app,
            2,
            Lyrics::Synced(parser::parse("[00:05]First\n[00:15]Second")),
        );
        app.set_player_elapsed(18.0);
        state.show_elapsed(&app);
        assert_eq!(app.get_lyrics_active(), 1);
        app.set_player_elapsed(6.0);
        state.show_elapsed(&app);
        assert_eq!(app.get_lyrics_active(), 0);
        app.set_player_paused(true);
        state.show_elapsed(&app);
        assert_eq!(app.get_lyrics_active(), 0);
    }

    #[test]
    fn stale_async_results_including_a_b_a_are_rejected() {
        let mut state = Presentation {
            generation: 1,
            ..Default::default()
        };
        assert!(state.accepts(1));
        state.generation = 2; // B replaces A before A finishes.
        assert!(!state.accepts(1));
        state.generation = 3; // Returning to A must not resurrect its earlier request.
        assert!(!state.accepts(1));
        assert!(!state.accepts(2));
        assert!(state.accepts(3));
    }
}
