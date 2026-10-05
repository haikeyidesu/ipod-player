slint::include_modules!();

mod artwork;
mod library;
mod lyrics;
mod mpd;
mod platform;
#[cfg(any(target_os = "macos", test))]
mod window_settings;

enum StatusCommand {
    Transport(mpd::Transport),
    Seek {
        id: u64,
        seconds: f64,
    },
    Volume {
        value: u8,
        app: slint::Weak<AppWindow>,
    },
    Refresh,
}

impl StatusCommand {
    fn execute(self) {
        let result = match self {
            Self::Transport(command) => mpd::send(command),
            Self::Seek { id, seconds } => mpd::seek(id, seconds),
            Self::Volume { value, app } => {
                let result = mpd::set_playback(mpd::PlaybackSetting::Volume(value));
                // MPD may clamp the requested value. Only its acknowledged,
                // read-back mixer value is eligible for user feedback.
                let confirmed = if result.is_ok() {
                    mpd::read_status()
                        .ok()
                        .and_then(|state| state.volume)
                        .unwrap_or(-1)
                } else {
                    -1
                };
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = app.upgrade() {
                        app.invoke_volume_completed(i32::from(value), confirmed);
                    }
                });
                result
            }
            Self::Refresh => Ok(()),
        };
        if let Err(err) = result {
            eprintln!("MPD command: {err}");
        }
    }
}

#[cfg(target_os = "macos")]
#[path = "platform/macos.rs"]
mod macos;

#[cfg(target_os = "macos")]
#[path = "platform/battery.rs"]
mod battery;

/// Elapsed/metadata changes do not invalidate the browser's settings models.
fn settings_changed(previous: Option<&mpd::PlayerState>, current: &mpd::PlayerState) -> bool {
    previous.is_none_or(|old| {
        old.volume != current.volume
            || old.crossfade != current.crossfade
            || old.repeat != current.repeat
            || old.random != current.random
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if lyrics::export_command()? {
        return Ok(());
    }
    let app = AppWindow::new()?;
    // One worker serializes transport commands and reads state. Library requests
    // run on their own worker and can request an immediate state refresh.
    let (sender, receiver) = std::sync::mpsc::channel::<StatusCommand>();
    #[cfg(target_os = "macos")]
    let _now_playing = platform::macos_now_playing::Bridge::install(sender.clone())?;
    library::install(&app, sender.clone());
    let artwork_sender = artwork::start(&app);
    let lyrics = lyrics::LyricsService::install(&app);
    let lyrics_weak = app.as_weak();
    app.on_refresh_lyrics(move || {
        if let Some(app) = lyrics_weak.upgrade() {
            let track = lyrics::Track::new(
                &app.get_player_file(),
                &app.get_player_title(),
                &app.get_player_artist(),
                &app.get_player_album(),
                app.get_player_duration() as f64,
            );
            lyrics.update(&app, track, &app.get_player_song_id());
        }
    });
    let weak = app.as_weak();
    std::thread::spawn(move || {
        use std::{sync::mpsc::RecvTimeoutError, time::Duration};
        let mut last_error = None;
        let mut last_snapshot: Option<mpd::PlayerState> = None;
        loop {
            // Refresh at playback speed while playing; check external changes less often
            // when paused. Commands always trigger an immediate refresh.
            match receiver.try_recv() {
                Ok(command) => command.execute(),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            let state = match mpd::read_player_state() {
                Ok(state) => {
                    last_error = None;
                    state
                }
                Err(err) => {
                    if last_error.as_ref() != Some(&err) {
                        eprintln!("MPD state: {err}");
                        last_error = Some(err);
                    }
                    // Clear stale song data if the server is unavailable.
                    mpd::PlayerState::default()
                }
            };
            let playing = state.playing;
            if last_snapshot.as_ref() != Some(&state) {
                let queue_changed = last_snapshot.as_ref().map(|old| old.queue_version)
                    != Some(state.queue_version);
                let refresh_settings = settings_changed(last_snapshot.as_ref(), &state);
                last_snapshot = Some(state.clone());
                let weak = weak.clone();
                let artwork_sender = artwork_sender.clone();
                if slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        #[cfg(target_os = "macos")]
                        platform::macos_now_playing::publish(&state);
                        if app.get_player_file().as_str() != state.file {
                            app.set_player_file(state.file.clone().into());
                            app.set_player_art(slint::Image::default());
                            app.set_player_has_art(false);
                            if !state.file.is_empty() {
                                let _ = artwork_sender.send(state.file.clone());
                            }
                        }
                        app.set_player_elapsed_label(artwork::time_label(state.elapsed).into());
                        app.set_player_remaining_label(if state.duration > 0.0 {
                            format!(
                                "-{}",
                                artwork::time_label((state.duration - state.elapsed).max(0.0))
                            )
                            .into()
                        } else {
                            "--:--".into()
                        });
                        let song_id = state.song_id.map(|id| id.to_string()).unwrap_or_default();
                        if app.get_player_song_id().as_str() != song_id
                            || !(state.playing || state.paused)
                        {
                            app.set_scrubbing(false);
                        }
                        app.set_player_song_id(song_id.into());
                        app.set_player_queue_length(
                            state
                                .queue_length
                                .and_then(|n| i32::try_from(n).ok())
                                .unwrap_or(0),
                        );
                        app.set_player_title(state.title.into());
                        app.set_player_artist(state.artist.into());
                        app.set_player_album(state.album.into());
                        app.set_player_elapsed(state.elapsed as f32);
                        app.set_player_duration(state.duration as f32);
                        app.invoke_refresh_lyrics();
                        app.set_player_playing(state.playing);
                        app.set_player_paused(state.paused);
                        if app.get_volume_target() < 0
                            || app.get_volume_target() == state.volume.unwrap_or(-1)
                        {
                            app.set_player_volume(state.volume.unwrap_or(-1));
                            if app.get_volume_target() == state.volume.unwrap_or(-1) {
                                app.set_volume_target(-1);
                            }
                        }
                        app.set_player_crossfade(state.crossfade as f32);
                        app.set_player_repeat(state.repeat);
                        app.set_player_random(state.random);
                        if refresh_settings {
                            app.invoke_refresh_settings();
                        }
                        app.set_player_queue_position(
                            state
                                .queue_position
                                .and_then(|pos| i32::try_from(pos).ok())
                                .unwrap_or(-1),
                        );
                        if queue_changed {
                            app.invoke_refresh_queue();
                        }
                    }
                })
                .is_err()
                {
                    break;
                }
            }
            let delay = if playing {
                Duration::from_secs(1)
            } else {
                Duration::from_secs(5)
            };
            match receiver.recv_timeout(delay) {
                Ok(command) => command.execute(),
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    });
    let volume_sender = sender.clone();
    let volume_weak = app.as_weak();
    app.on_volume_requested(move |value| {
        if (0..=100).contains(&value)
            && let Some(app) = volume_weak.upgrade()
        {
            // Keep successive key presses relative to the latest requested value.
            app.set_volume_target(value);
            if volume_sender
                .send(StatusCommand::Volume {
                    value: value as u8,
                    app: volume_weak.clone(),
                })
                .is_err()
            {
                app.set_volume_target(-1);
                eprintln!("MPD command worker stopped");
            }
        }
    });
    let completion_weak = app.as_weak();
    app.on_volume_completed(move |requested, confirmed| {
        if let Some(app) = completion_weak.upgrade() {
            if app.get_volume_target() == requested {
                app.set_volume_target(-1);
            }
            if confirmed >= 0 {
                app.set_player_volume(confirmed);
                app.invoke_show_status(if confirmed == 0 {
                    "Muted".into()
                } else {
                    format!("Volume {confirmed}%").into()
                });
            }
        }
    });
    let seek_sender = sender.clone();
    app.on_seek_requested(move |id, seconds| {
        if let Ok(id) = id.parse::<u64>()
            && seek_sender
                .send(StatusCommand::Seek {
                    id,
                    seconds: seconds as f64,
                })
                .is_err()
        {
            eprintln!("MPD command worker stopped");
        }
    });
    app.on_transport_action(move |action| {
        if let Some(command) = mpd::Transport::from_ui(&action)
            && sender.send(StatusCommand::Transport(command)).is_err()
        {
            eprintln!("MPD command worker stopped");
        }
    });

    #[cfg(target_os = "macos")]
    {
        use slint::{ComponentHandle, RenderingState};

        // Keep the event monitor alive for as long as the Slint window exists.
        let settings = macos::prepare(&app);
        let weak = app.as_weak();
        let monitor = std::rc::Rc::new(std::cell::RefCell::new(None));
        let resize_monitor = monitor.clone();
        app.window().set_rendering_notifier(move |state, _| {
            if matches!(state, RenderingState::RenderingSetup) && resize_monitor.borrow().is_none()
            {
                let Some(app) = weak.upgrade() else {
                    eprintln!("Cannot configure iPod window: Slint component was destroyed");
                    std::process::exit(1);
                };
                match macos::install(&app, settings.clone()) {
                    Ok(monitor) => {
                        *resize_monitor.borrow_mut() = Some(monitor);
                        println!("macOS aspect ratio lock and resize monitor installed");
                    }
                    Err(err) => {
                        eprintln!("Cannot configure iPod window: {err}");
                        std::process::exit(1);
                    }
                }
            }
        })?;
        app.show()?;
        let _battery_timer = battery::start(app.as_weak());
        let result = slint::run_event_loop();
        // Flush a final move/resize even when quitting inside the debounce period.
        monitor.borrow_mut().take();
        result?;
    }

    #[cfg(not(target_os = "macos"))]
    app.run()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_refresh_ignores_playback_ticks_but_tracks_all_settings() {
        let previous = mpd::PlayerState::default();
        assert!(settings_changed(None, &previous));
        let mut current = previous.clone();
        current.elapsed = 12.0;
        current.title = "New track".into();
        current.playing = true;
        assert!(!settings_changed(Some(&previous), &current));
        for changed in [
            mpd::PlayerState {
                volume: Some(42),
                ..previous.clone()
            },
            mpd::PlayerState {
                crossfade: 5.0,
                ..previous.clone()
            },
            mpd::PlayerState {
                repeat: true,
                ..previous.clone()
            },
            mpd::PlayerState {
                random: true,
                ..previous.clone()
            },
        ] {
            assert!(settings_changed(Some(&previous), &changed));
            assert!(settings_changed(Some(&changed), &previous));
        }
    }
}
