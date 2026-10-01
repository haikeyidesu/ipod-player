//! MediaPlayer consumes the same snapshots as Slint; it never connects to MPD.
//! Install, publish and drop on the main thread. Slint supplies the Cocoa run loop.
use std::{marker::PhantomData, rc::Rc, sync::mpsc::Sender};

use mediaplayer::{
    MediaPlayerError,
    now_playing::{NowPlayingInfo, NowPlayingInfoCenter, NowPlayingMediaType, PlaybackState},
    remote_commands::{Command, CommandToken, HandlerStatus, RemoteCommandCenter},
};

use crate::{
    StatusCommand,
    mpd::{PlayerState, Transport},
};

const COMMANDS: [(Command, Transport); 5] = [
    (Command::Play, Transport::Play),
    (Command::Pause, Transport::Pause),
    (Command::TogglePlayPause, Transport::PlayPause),
    (Command::NextTrack, Transport::Next),
    (Command::PreviousTrack, Transport::Previous),
];

/// Own registrations until the app exits, including cleanup on partial install.
pub struct Bridge {
    tokens: Vec<CommandToken>,
    _main_thread: PhantomData<Rc<()>>,
}

impl Bridge {
    pub fn install(sender: Sender<StatusCommand>) -> Result<Self, MediaPlayerError> {
        let center = RemoteCommandCenter::shared();
        let mut bridge = Self {
            tokens: Vec::new(),
            _main_thread: PhantomData,
        };
        for (command, transport) in COMMANDS {
            let sender = sender.clone();
            let remote = center.command(command);
            bridge
                .tokens
                .push(remote.add_handler(move |_| enqueue(&sender, transport))?);
            remote.set_enabled(true);
        }
        Ok(bridge)
    }
}

// Callbacks may arrive off-main-thread: enqueue only, never block on MPD or touch UI.
// Success means accepted by the worker; MPD errors are reported by that worker.
fn enqueue(sender: &Sender<StatusCommand>, transport: Transport) -> HandlerStatus {
    if sender.send(StatusCommand::Transport(transport)).is_ok() {
        HandlerStatus::Success
    } else {
        HandlerStatus::CommandFailed
    }
}

fn playback(state: &PlayerState) -> PlaybackState {
    if state.queue_position.is_none() {
        PlaybackState::Stopped
    } else if state.playing {
        PlaybackState::Playing
    } else if state.paused {
        PlaybackState::Paused
    } else {
        PlaybackState::Stopped
    }
}

/// Called from the same main-loop closure that applies this snapshot to Slint.
/// At most the existing worker cadence (1 Hz playing, 0.2 Hz paused), plus commands.
/// macOS interpolates elapsed time using the published rate between snapshots.
pub fn publish(state: &PlayerState) {
    let center = NowPlayingInfoCenter::default_center();
    let playback = playback(state);
    if playback == PlaybackState::Stopped {
        center.set_playback_state(playback);
        center.clear();
        return;
    }
    let info = NowPlayingInfo::new()
        .title(&state.title)
        .artist(&state.artist)
        .album_title(&state.album)
        .playback_duration(state.duration)
        .elapsed_playback_time(state.elapsed)
        .playback_rate(if state.playing { 1.0 } else { 0.0 })
        .default_playback_rate(1.0)
        .media_type(NowPlayingMediaType::Audio);
    if let Err(err) = center.set_now_playing_info(&info) {
        eprintln!("macOS Now Playing metadata: {err}");
    }
    // Required on macOS independently of playback rate; retain metadata on pause.
    center.set_playback_state(playback);
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.tokens.clear();
        let commands = RemoteCommandCenter::shared();
        for (command, _) in COMMANDS {
            commands.command(command).set_enabled(false);
        }
        let center = NowPlayingInfoCenter::default_center();
        center.set_playback_state(PlaybackState::Stopped);
        center.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_play_pause_stop_and_missing_song() {
        let mut state = PlayerState {
            queue_position: Some(0),
            playing: true,
            ..Default::default()
        };
        assert_eq!(playback(&state), PlaybackState::Playing);
        state.playing = false;
        state.paused = true;
        assert_eq!(playback(&state), PlaybackState::Paused);
        state.paused = false;
        assert_eq!(playback(&state), PlaybackState::Stopped);
        state.playing = true;
        state.queue_position = None;
        assert_eq!(playback(&state), PlaybackState::Stopped);
    }

    #[test]
    fn all_remote_commands_use_existing_transport_channel() {
        let (sender, receiver) = std::sync::mpsc::channel();
        for (_, transport) in COMMANDS {
            assert_eq!(enqueue(&sender, transport), HandlerStatus::Success);
            assert!(
                matches!(receiver.recv().unwrap(), StatusCommand::Transport(value) if value == transport)
            );
        }
        drop(receiver);
        assert_eq!(
            enqueue(&sender, Transport::Play),
            HandlerStatus::CommandFailed
        );
    }
}
