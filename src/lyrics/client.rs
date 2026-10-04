//! Sequential, bounded LRCLIB requests with conservative recording matching.
use super::{Lyrics, Track};
use reqwest::{StatusCode, blocking::Client as HttpClient};
use serde::{Deserialize, de::DeserializeOwned};
use std::{
    io::Read,
    time::{Duration, Instant, SystemTime},
};

const API: &str = "https://lrclib.net/api";
const MAX_BODY: u64 = 2 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    track_name: String,
    artist_name: String,
    #[serde(default)]
    album_name: String,
    duration: f64,
    #[serde(default)]
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

fn normalized(value: &str) -> String {
    // Do NOT strip remix/live/cover qualifiers, punctuation or featured artists.
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
impl Record {
    fn matches(&self, track: &Track) -> bool {
        normalized(&self.track_name) == normalized(&track.title)
            && normalized(&self.artist_name) == normalized(&track.artist)
            && (track.album.trim().is_empty()
                || normalized(&self.album_name) == normalized(&track.album))
            && (track.duration_ms == 0
                || (self.duration.is_finite()
                    && (self.duration - track.duration_ms as f64 / 1000.0).abs() <= 2.0))
    }
    fn lyrics(&self) -> Lyrics {
        Lyrics::from_text(
            self.synced_lyrics.as_deref(),
            self.plain_lyrics.as_deref(),
            self.instrumental,
        )
    }
}

fn choose(records: Vec<Record>, track: &Track) -> Lyrics {
    // Keep the parsed result alongside its metadata: filtering and ranking must
    // not repeatedly parse/sort the same LRC transcript.
    let candidates: Vec<_> = records
        .into_iter()
        .filter(|r| r.matches(track))
        .filter_map(|record| {
            let lyrics = record.lyrics();
            (lyrics != Lyrics::Missing).then_some((record, lyrics))
        })
        .collect();
    // Missing disambiguating metadata must not choose arbitrarily among releases
    // or materially different durations, even if one happens to be synced.
    if candidates.iter().any(|(a, _)| {
        candidates.iter().any(|(b, _)| {
            (track.album.trim().is_empty()
                && normalized(&a.album_name) != normalized(&b.album_name))
                || (a.duration - b.duration).abs() > 2.0
                || a.instrumental != b.instrumental
        })
    }) {
        return Lyrics::Missing;
    }
    candidates
        .into_iter()
        .min_by_key(|(_, lyrics)| match lyrics {
            Lyrics::Synced(_) => 0,
            Lyrics::Plain(_) => 1,
            _ => 2,
        })
        .map(|(_, lyrics)| lyrics)
        .unwrap_or(Lyrics::Missing)
}

fn retry_delay(value: Option<&str>, now: SystemTime) -> Duration {
    value
        .and_then(|s| {
            s.parse::<u64>().ok().map(Duration::from_secs).or_else(|| {
                httpdate::parse_http_date(s)
                    .ok()
                    .map(|date| date.duration_since(now).unwrap_or_default())
            })
        })
        .unwrap_or(Duration::from_secs(60))
}

pub struct Client {
    http: HttpClient,
    next_request: Instant,
    blocked_until: SystemTime,
}
impl Client {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            http: HttpClient::builder()
                .user_agent(concat!(
                    "iPod Player/",
                    env!("CARGO_PKG_VERSION"),
                    " (https://github.com/haikeyidesu/ipod-player)"
                ))
                .timeout(Duration::from_secs(12))
                .connect_timeout(Duration::from_secs(4))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| e.to_string())?,
            next_request: Instant::now(),
            blocked_until: SystemTime::now(),
        })
    }
    fn get<T: DeserializeOwned>(
        &mut self,
        route: &str,
        query: &[(String, String)],
        valid: &impl Fn() -> bool,
    ) -> Result<Option<T>, String> {
        if SystemTime::now() < self.blocked_until {
            return Err("LRCLIB cooldown active; using cached lyrics only".into());
        }
        std::thread::sleep(self.next_request.saturating_duration_since(Instant::now()));
        if !valid() {
            return Err("obsolete track request cancelled".into());
        }
        let response = self.http.get(format!("{API}/{route}")).query(query).send();
        self.next_request = Instant::now() + Duration::from_millis(500);
        let response =
            response.map_err(|e| format!("LRCLIB connection failed: {}", e.without_url()))?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS
            || response.status() == StatusCode::SERVICE_UNAVAILABLE
        {
            let now = SystemTime::now();
            let delay = retry_delay(
                response
                    .headers()
                    .get("Retry-After")
                    .and_then(|s| s.to_str().ok()),
                now,
            );
            self.blocked_until = now
                .checked_add(delay)
                .unwrap_or(now + Duration::from_secs(365 * 24 * 3600));
            return Err("LRCLIB temporarily unavailable; respecting Retry-After".into());
        }
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(format!("LRCLIB HTTP {}", response.status()));
        }
        if !response
            .headers()
            .get("Content-Type")
            .and_then(|s| s.to_str().ok())
            .is_some_and(|s| s.starts_with("application/json"))
        {
            return Err("LRCLIB returned a non-JSON response".into());
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_BODY + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_BODY {
            return Err("LRCLIB response exceeds size limit".into());
        }
        self.next_request = Instant::now() + Duration::from_millis(500);
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| "Invalid LRCLIB JSON".into())
    }
    pub fn fetch(&mut self, track: &Track, valid: &impl Fn() -> bool) -> Result<Lyrics, String> {
        if track.title.trim().is_empty() || track.artist.trim().is_empty() {
            return Ok(Lyrics::Missing);
        }
        let mut query = vec![
            ("track_name".into(), track.title.clone()),
            ("artist_name".into(), track.artist.clone()),
        ];
        if !track.album.trim().is_empty() {
            query.push(("album_name".into(), track.album.clone()));
        }
        let mut exact_query = query.clone();
        let duration = track.duration_ms as f64 / 1000.0;
        if (1.0..=3600.0).contains(&duration) {
            exact_query.push(("duration".into(), duration.to_string()));
        }
        let exact = self
            .get::<Record>("get", &exact_query, valid)?
            .filter(|record| record.matches(track))
            .map(|r| r.lyrics())
            .unwrap_or(Lyrics::Missing);
        // An acceptable exact match (including plain or instrumental) avoids a
        // needless search. Search only when no suitable exact lyrics exist.
        if exact != Lyrics::Missing {
            return Ok(exact);
        }
        Ok(choose(
            self.get::<Vec<Record>>("search", &query, valid)?
                .unwrap_or_default(),
            track,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(title: &str, album: &str, synced: bool) -> Record {
        Record {
            track_name: title.into(),
            artist_name: "Artist".into(),
            album_name: album.into(),
            duration: 120.0,
            instrumental: false,
            plain_lyrics: Some("Words".into()),
            synced_lyrics: synced.then(|| "[00:01]Words".into()),
        }
    }
    #[test]
    fn conservative_matching_and_synced_preference() {
        let track = Track::new("f", "Song", "Artist", "Album", 120.0);
        assert!(!record("Song (Remix)", "Album", true).matches(&track));
        let mut cover = record("Song", "Album", true);
        cover.artist_name = "Cover Artist".into();
        assert!(!cover.matches(&track));
        let mut longer = record("Song", "Album", true);
        longer.duration = 130.0;
        assert!(!longer.matches(&track));
        assert!(matches!(
            choose(
                vec![
                    record("Song", "Album", false),
                    record("Song", "Album", true)
                ],
                &track
            ),
            Lyrics::Synced(_)
        ));
        let unknown = Track::new("f", "Song", "Artist", "", 0.0);
        assert_eq!(
            choose(
                vec![
                    record("Song", "Album", false),
                    record("Song", "Other", true)
                ],
                &unknown
            ),
            Lyrics::Missing
        );
    }
    #[test]
    #[ignore = "explicit opt-in: performs a single public LRCLIB lookup"]
    fn live_lrclib_documented_example() {
        let mut client = Client::new().unwrap();
        let track = Track::new(
            "fixture",
            "I Want to Live",
            "Borislav Slavov",
            "Baldur's Gate 3 (Original Game Soundtrack)",
            233.0,
        );
        let lyrics = client.fetch(&track, &|| true).unwrap();
        assert!(matches!(lyrics, Lyrics::Synced(_) | Lyrics::Plain(_)));
    }

    #[test]
    fn selection_keeps_first_tie_and_rejects_ambiguous_recordings() {
        let track = Track::new("f", "Song", "Artist", "Album", 120.0);
        let first = record("Song", "Album", false);
        let mut second = record("Song", "Album", false);
        second.plain_lyrics = Some("Other words".into());
        assert_eq!(
            choose(vec![first, second], &track),
            Lyrics::Plain("Words".into())
        );
        let mut missing = record("Song", "Album", false);
        missing.plain_lyrics = None;
        assert_eq!(choose(vec![missing], &track), Lyrics::Missing);
        let mut instrumental = record("Song", "Album", false);
        instrumental.instrumental = true;
        assert_eq!(
            choose(vec![record("Song", "Album", true), instrumental], &track),
            Lyrics::Missing
        );
    }

    #[test]
    fn retry_after_seconds_date_and_invalid() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert_eq!(retry_delay(Some("120"), now), Duration::from_secs(120));
        let date = httpdate::fmt_http_date(now + Duration::from_secs(90));
        assert_eq!(retry_delay(Some(&date), now), Duration::from_secs(90));
        assert_eq!(retry_delay(Some("invalid"), now), Duration::from_secs(60));
    }
}
