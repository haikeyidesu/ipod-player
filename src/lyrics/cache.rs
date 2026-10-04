//! Durable per-user cache; never reads or modifies original music files.
use super::{Lyrics, Track};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const MISS_TTL: u64 = 24 * 60 * 60;

#[derive(Serialize, Deserialize)]
struct Entry {
    version: u32,
    track: Track,
    saved: u64,
    lyrics: Lyrics,
}

pub struct Cache {
    root: PathBuf,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn key(track: &Track) -> String {
    // Structured serialization prevents separator collisions; full path and exact
    // metadata distinguish recordings. Filenames never expose track metadata.
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(track).expect("serializable track"))
    )
}

impl Cache {
    pub fn application() -> Option<Self> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        #[cfg(target_os = "macos")]
        let root =
            home.join("Library/Application Support/io.github.haikeyidesu.ipod-player/lyrics-v1");
        #[cfg(not(target_os = "macos"))]
        let root = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("ipod-player/lyrics-v1");
        Some(Self { root })
    }
    pub fn load(&self, track: &Track) -> Option<Lyrics> {
        let file = fs::File::open(self.root.join(format!("{}.json", key(track)))).ok()?;
        if file.metadata().ok()?.len() > MAX_BYTES {
            return None;
        }
        let entry: Entry = serde_json::from_reader(file.take(MAX_BYTES)).ok()?;
        if entry.version != 1 || entry.track != *track {
            return None;
        }
        if matches!(entry.lyrics, Lyrics::Missing) && now().saturating_sub(entry.saved) >= MISS_TTL
        {
            return None;
        }
        Some(entry.lyrics)
    }
    pub fn save(&self, track: &Track, lyrics: &Lyrics) -> io::Result<()> {
        fs::create_dir_all(&self.root)?;
        let key = key(track);
        let target = self.root.join(format!("{key}.json"));
        let temporary = self.root.join(format!(
            "{key}.{}.{:016x}.tmp",
            std::process::id(),
            rand::random::<u64>()
        ));
        let bytes = serde_json::to_vec(&Entry {
            version: 1,
            track: track.clone(),
            saved: now(),
            lyrics: lyrics.clone(),
        })?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::other("lyrics exceed cache limit"));
        }
        let result = (|| {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, target)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offline_round_trip_collision_and_corruption() {
        let root =
            std::env::temp_dir().join(format!("ipod-lyrics-test-{:016x}", rand::random::<u64>()));
        let cache = Cache { root: root.clone() };
        let track = Track::new("folder/song.flac", "Song", "Artist", "Album", 123.0);
        let other = Track::new("different/song.flac", "Song", "Artist", "Album", 123.0);
        assert_ne!(key(&track), key(&other));
        assert_eq!(key(&track), key(&track.clone()));
        assert_eq!(cache.load(&track), None);
        let lyrics = Lyrics::Plain("Offline words".into());
        cache.save(&track, &lyrics).unwrap();
        assert_eq!(cache.load(&track), Some(lyrics));
        assert_eq!(cache.load(&other), None);
        fs::write(root.join(format!("{}.json", key(&track))), b"broken").unwrap();
        assert_eq!(cache.load(&track), None);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn expired_misses_are_retried_but_downloads_are_kept() {
        let root =
            std::env::temp_dir().join(format!("ipod-lyrics-expiry-{:016x}", rand::random::<u64>()));
        fs::create_dir_all(&root).unwrap();
        let cache = Cache { root: root.clone() };
        let track = Track::new("song", "Song", "Artist", "", 0.0);
        for lyrics in [
            Lyrics::Missing,
            Lyrics::Instrumental,
            Lyrics::Plain("Words".into()),
        ] {
            let entry = Entry {
                version: 1,
                track: track.clone(),
                saved: 0,
                lyrics: lyrics.clone(),
            };
            fs::write(
                root.join(format!("{}.json", key(&track))),
                serde_json::to_vec(&entry).unwrap(),
            )
            .unwrap();
            assert_eq!(
                cache.load(&track),
                if lyrics == Lyrics::Missing {
                    None
                } else {
                    Some(lyrics)
                }
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
