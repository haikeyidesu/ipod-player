//! Authoritative sidecars. No provider downloads are written here automatically.
//! The configured root is trusted; reject symlinks below it and untrusted URI paths.
use super::{Lyrics, parser};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_BYTES: u64 = 2 * 1024 * 1024;

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn configured() -> Option<Self> {
        std::env::var_os("IPOD_MUSIC_DIR").map(|root| Self::new(root.into()))
    }

    /// MPD database URIs are slash-separated paths, not URLs. Never percent-decode
    /// them: `%2e%2e` may be a literal directory name, not a traversal instruction.
    pub fn path(&self, uri: &str) -> Result<PathBuf, String> {
        if uri.is_empty()
            || uri.contains(['\\', ':', '\0', '\r', '\n'])
            || uri
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err("Not a safe relative MPD song URI".into());
        }
        if !self.root.is_absolute() {
            return Err("IPOD_MUSIC_DIR must be an absolute path".into());
        }
        let root = self
            .root
            .canonicalize()
            .map_err(|e| format!("Music directory unavailable: {e}"))?;
        let mut audio = root;
        let parts: Vec<_> = uri.split('/').collect();
        for (index, part) in parts.iter().enumerate() {
            audio.push(part);
            let metadata =
                fs::symlink_metadata(&audio).map_err(|e| format!("Music path unavailable: {e}"))?;
            if metadata.file_type().is_symlink()
                || (index + 1 < parts.len() && !metadata.is_dir())
                || (index + 1 == parts.len() && !metadata.is_file())
            {
                return Err("Music path must use real directories and a regular audio file".into());
            }
        }
        let sidecar = audio.with_extension("lrc");
        if sidecar == audio {
            return Err("Refusing to treat an audio URI as its own lyrics sidecar".into());
        }
        Ok(sidecar)
    }

    /// Existing but unreadable/invalid files are errors, not misses: never hide
    /// an authoritative local file behind a provider result.
    pub fn load(&self, uri: &str) -> Result<Option<Lyrics>, String> {
        let path = self.path(uri)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Cannot inspect local lyrics: {e}")),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_BYTES {
            return Err("Local lyrics must be a regular UTF-8 file of at most 2 MiB".into());
        }
        let mut bytes = Vec::new();
        fs::File::open(path)
            .and_then(|file| file.take(MAX_BYTES + 1).read_to_end(&mut bytes))
            .map_err(|e| format!("Cannot read local lyrics: {e}"))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("Local lyrics exceed 2 MiB".into());
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| "Local lyrics are not UTF-8")?;
        let text = text.trim_start_matches('\u{feff}');
        Ok(Some(Lyrics::from_text(Some(text), Some(text), false)))
    }

    /// Publish a complete file with an atomic, no-replace hard link, not rename
    /// (which could overwrite a file created after the preflight). Unsupported
    /// filesystems fail safely; no truncate/copy fallback is allowed.
    pub fn export(&self, uri: &str, lyrics: &Lyrics, write: bool) -> Result<PathBuf, String> {
        let text = export_text(lyrics)?;
        let target = self.path(uri)?;
        match fs::symlink_metadata(&target) {
            Ok(_) => return Err("Local lyrics already exist; refusing to overwrite".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        if write {
            publish(&target, text.as_bytes()).map_err(|e| format!("Lyrics export failed: {e}"))?;
        }
        Ok(target)
    }
}

fn publish(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary =
        target.with_file_name(format!(".ipod-lyrics-{:016x}.tmp", rand::random::<u64>()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::hard_link(&temporary, target)
    })();
    drop(file);
    let _ = fs::remove_file(temporary);
    result
}

/// v1 stored parsed lines, not source LRC. Export only representable synced data,
/// normalizing timestamps to milliseconds and repeating timestamps for translations.
fn export_text(lyrics: &Lyrics) -> Result<String, String> {
    let Lyrics::Synced(lines) = lyrics else {
        return Err("Only synced lyrics can be exported as LRC".into());
    };
    if lines.is_empty() || !lines.iter().any(|line| !line.text.trim().is_empty()) {
        return Err("No synced words to export".into());
    }
    let mut output = String::new();
    let mut previous = 0.0;
    for line in lines {
        if !line.timestamp.is_finite()
            || line.timestamp < previous
            || line.timestamp > 86400.0
            || line.text.contains(['\r', '\0'])
        {
            return Err("Cached lyrics contain unsuitable timestamps or text".into());
        }
        previous = line.timestamp;
        let ms = (line.timestamp * 1000.0).round() as u64;
        for text in line.text.split('\n') {
            if text.starts_with('[') {
                return Err("Cached text could be interpreted as an LRC tag".into());
            }
            output.push_str(&format!(
                "[{:02}:{:02}.{:03}]{text}\n",
                ms / 60000,
                ms / 1000 % 60,
                ms % 1000
            ));
        }
        if output.len() as u64 > MAX_BYTES {
            return Err("Export exceeds 2 MiB".into());
        }
    }
    // Exercise the same standard parser used at playback before publishing.
    if parser::parse(&output).is_empty() {
        return Err("Export did not produce valid LRC".into());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn fixture() -> (PathBuf, Store) {
        let root =
            std::env::temp_dir().join(format!("ipod-sidecars-{:016x}", rand::random::<u64>()));
        fs::create_dir_all(root.join("Album")).unwrap();
        fs::write(root.join("Album/Song.flac"), []).unwrap();
        (root.clone(), Store::new(root))
    }

    #[test]
    fn paths_are_relative_literal_and_confined() {
        let (root, store) = fixture();
        assert_eq!(
            store.path("Album/Song.flac").unwrap(),
            root.canonicalize().unwrap().join("Album/Song.lrc")
        );
        for uri in [
            "",
            "/etc/passwd",
            "../song.flac",
            "Album/../Song.flac",
            "./Album/Song.flac",
            "Album//Song.flac",
            "https://host/song",
            "C:\\song.flac",
            "Album/Song\0.flac",
            "Album/",
        ] {
            assert!(store.path(uri).is_err(), "{uri:?}");
        }
        fs::write(root.join("Album/日本 %20.flac"), []).unwrap();
        assert!(
            store
                .path("Album/日本 %20.flac")
                .unwrap()
                .ends_with("日本 %20.lrc")
        );
        assert!(
            Store::new("relative".into())
                .path("Album/Song.flac")
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reload_utf8_plain_empty_and_invalid_files() {
        let (root, store) = fixture();
        let uri = "Album/Song.flac";
        let path = store.path(uri).unwrap();
        assert_eq!(store.load(uri).unwrap(), None);
        fs::write(&path, "\u{feff}[offset:500]\r\n[00:01.250]こんにちは\r\n").unwrap();
        assert_eq!(
            store.load(uri).unwrap(),
            Some(Lyrics::Synced(parser::parse("[00:01.750]こんにちは")))
        );
        fs::write(&path, "Edited plain words").unwrap();
        assert_eq!(
            store.load(uri).unwrap(),
            Some(Lyrics::Plain("Edited plain words".into()))
        );
        fs::write(&path, "").unwrap();
        assert_eq!(store.load(uri).unwrap(), Some(Lyrics::Missing));
        fs::write(&path, [0xff]).unwrap();
        assert!(store.load(uri).is_err());
        fs::remove_file(path).unwrap();
        assert_eq!(store.load(uri).unwrap(), None);
        fs::remove_dir_all(root).unwrap();
        assert!(store.load(uri).is_err());
    }

    #[test]
    fn export_is_dry_run_first_atomic_and_never_replaces() {
        let (root, store) = fixture();
        let lyrics = Lyrics::Synced(parser::parse(
            "[00:01.234]One\n[00:01.234]Translation\n[00:02]Two",
        ));
        let target = store.export("Album/Song.flac", &lyrics, false).unwrap();
        assert!(!target.exists());
        store.export("Album/Song.flac", &lyrics, true).unwrap();
        assert_eq!(store.load("Album/Song.flac").unwrap(), Some(lyrics.clone()));
        assert!(store.export("Album/Song.flac", &lyrics, true).is_err());
        assert!(publish(&target, b"overwrite race").is_err());
        assert_eq!(store.load("Album/Song.flac").unwrap(), Some(lyrics));
        assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 2);
        assert!(export_text(&Lyrics::Plain("Words".into())).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unsuitable_cached_data_cannot_be_exported() {
        use super::super::parser::LyricLine;
        for timestamp in [-1.0, f64::NAN, f64::INFINITY, 86401.0] {
            assert!(
                export_text(&Lyrics::Synced(vec![LyricLine {
                    timestamp,
                    text: "Words".into()
                }]))
                .is_err()
            );
        }
        for text in ["[00:04]Injected", "a\rb", "a\0b"] {
            assert!(
                export_text(&Lyrics::Synced(vec![LyricLine {
                    timestamp: 1.0,
                    text: text.into()
                }]))
                .is_err()
            );
        }
        assert!(export_text(&Lyrics::Missing).is_err());
        assert!(export_text(&Lyrics::Instrumental).is_err());
        assert!(export_text(&Lyrics::Synced(vec![])).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn read_only_directory_loads_but_export_fails_without_temp_files() {
        use std::os::unix::fs::PermissionsExt;
        let (root, store) = fixture();
        let directory = root.join("Album");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o555)).unwrap();
        assert_eq!(store.load("Album/Song.flac").unwrap(), None);
        let lyrics = Lyrics::Synced(parser::parse("[00:01]Words"));
        let result = store.export("Album/Song.flac", &lyrics, true);
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_read_only_files_are_not_overwritten() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let (root, store) = fixture();
        symlink(root.join("Album"), root.join("Link")).unwrap();
        assert!(store.path("Link/Song.flac").is_err());
        symlink(root.join("Album/Song.flac"), root.join("linked.flac")).unwrap();
        assert!(store.path("linked.flac").is_err());
        symlink(root.parent().unwrap(), root.join("Outside")).unwrap();
        assert!(store.path("Outside/song.flac").is_err());
        let target = store.path("Album/Song.flac").unwrap();
        symlink(root.join("missing"), &target).unwrap();
        assert!(store.load("Album/Song.flac").is_err());
        let lyrics = Lyrics::Synced(parser::parse("[00:01]Words"));
        assert!(store.export("Album/Song.flac", &lyrics, true).is_err());
        fs::remove_file(&target).unwrap();
        fs::write(&target, "[00:01]Manual").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o444)).unwrap();
        assert!(store.load("Album/Song.flac").unwrap().is_some());
        assert!(store.export("Album/Song.flac", &lyrics, true).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
