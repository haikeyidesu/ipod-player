//! Source precedence independent of Slint and provider transport. Only the fallback
//! closure knows about cache/HTTP; local lyrics can never enter that cache.
use super::{Lyrics, Track, local::Store};

pub fn resolve(
    local: Option<&Store>,
    track: &Track,
    fallback: &mut Option<Result<Lyrics, String>>,
    fetch: impl FnOnce() -> Result<Lyrics, String>,
) -> Result<Lyrics, String> {
    let read_local = || match local {
        Some(store) => store.load(&track.file),
        None => Ok(None),
    };
    if let Some(lyrics) = read_local()? {
        return Ok(lyrics);
    }
    // Memoize errors too: the reload poll must not become a provider retry loop.
    if fallback.is_none() {
        *fallback = Some(fetch());
    }
    // An editor may have saved while the blocking provider lookup was in flight.
    // Recheck before delivery, including when the provider failed.
    if let Some(lyrics) = read_local()? {
        return Ok(lyrics);
    }
    fallback.as_ref().expect("fallback was resolved").clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn local_precedence_reload_deletion_and_inflight_creation() {
        let root =
            std::env::temp_dir().join(format!("ipod-resolver-{:016x}", rand::random::<u64>()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("song.flac"), []).unwrap();
        let store = Store::new(root.clone());
        let track = Track::new("song.flac", "Song", "Artist", "Album", 10.0);
        let path = root.join("song.lrc");
        fs::write(&path, "Manual").unwrap();
        let mut fallback = None;
        let never = || -> Result<Lyrics, String> { panic!("local must bypass cache and network") };
        assert_eq!(
            resolve(Some(&store), &track, &mut fallback, never).unwrap(),
            Lyrics::Plain("Manual".into())
        );
        assert!(fallback.is_none());
        fs::write(&path, "Edited").unwrap();
        assert_eq!(
            resolve(Some(&store), &track, &mut fallback, never).unwrap(),
            Lyrics::Plain("Edited".into())
        );
        fs::write(&path, "").unwrap();
        assert_eq!(
            resolve(Some(&store), &track, &mut fallback, never).unwrap(),
            Lyrics::Missing
        );
        fs::write(&path, [0xff]).unwrap();
        assert!(resolve(Some(&store), &track, &mut fallback, never).is_err());
        fs::remove_file(&path).unwrap();
        assert_eq!(
            resolve(Some(&store), &track, &mut fallback, || {
                fs::write(&path, "Saved during lookup").unwrap();
                Ok(Lyrics::Plain("Provider".into()))
            })
            .unwrap(),
            Lyrics::Plain("Saved during lookup".into())
        );
        fs::remove_file(&path).unwrap();
        assert_eq!(
            resolve(Some(&store), &track, &mut fallback, never).unwrap(),
            Lyrics::Plain("Provider".into())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unconfigured_local_uses_fallback_once_even_on_failure() {
        let track = Track::new("stream", "Song", "Artist", "", 0.0);
        let mut fallback = None;
        assert_eq!(
            resolve(None, &track, &mut fallback, || Err("offline".into())),
            Err("offline".into())
        );
        assert_eq!(
            resolve(None, &track, &mut fallback, || panic!(
                "must not retry on poll"
            )),
            Err("offline".into())
        );
    }
}
