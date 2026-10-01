//! Track-triggered artwork loading, not an independent MPD state poller.
use crate::{AppWindow, mpd};
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer};
use std::{collections::VecDeque, io::Cursor, sync::mpsc};

type Pixels = SharedPixelBuffer<Rgba8Pixel>;

fn decode(bytes: Vec<u8>) -> Result<Pixels, String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| e.to_string())?
        .thumbnail(256, 256)
        .to_rgba8();
    Ok(SharedPixelBuffer::clone_from_slice(
        image.as_raw(),
        image.width(),
        image.height(),
    ))
}

pub fn start(app: &AppWindow) -> mpsc::Sender<String> {
    let (tx, rx) = mpsc::channel::<String>();
    let weak = app.as_weak();
    std::thread::spawn(move || {
        // Cache successful images AND misses. Bounded to eight 256px images.
        let mut cache: VecDeque<(String, Option<Pixels>)> = VecDeque::new();
        while let Ok(mut file) = rx.recv() {
            while let Ok(newer) = rx.try_recv() {
                file = newer;
            }
            let pixels = if let Some(index) = cache.iter().position(|(key, _)| key == &file) {
                let entry = cache.remove(index).unwrap();
                let pixels = entry.1.clone();
                cache.push_back(entry);
                pixels
            } else {
                let pixels = [false, true]
                    .into_iter()
                    .find_map(|embedded| mpd::artwork(&file, embedded).and_then(decode).ok());
                cache.push_back((file.clone(), pixels.clone()));
                if cache.len() > 8 {
                    cache.pop_front();
                }
                pixels
            };
            let weak = weak.clone();
            if slint::invoke_from_event_loop(move || {
                if let Some(app) = weak.upgrade() {
                    // A late response for an old track must never replace new artwork.
                    if app.get_player_file().as_str() != file {
                        return;
                    }
                    app.set_player_has_art(pixels.is_some());
                    app.set_player_art(
                        pixels
                            .map(|p| slint::Image::from_rgba8(p))
                            .unwrap_or_default(),
                    );
                }
            })
            .is_err()
            {
                break;
            }
        }
    });
    tx
}

pub fn time_label(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.0).min(u32::MAX as f64) as u32
    } else {
        0
    };
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn time_and_invalid_art_are_safe() {
        assert_eq!(time_label(125.9), "2:05");
        assert_eq!(time_label(f64::NAN), "0:00");
        assert_eq!(time_label(-1.0), "0:00");
        assert!(decode(vec![1, 2, 3]).is_err());
    }
}
