//! UI-side browser controller. All MPD requests and mutations run on one worker.
use crate::{AppWindow, StatusCommand, mpd};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
};

const PAGE: usize = 6;

#[derive(Clone, Debug, PartialEq, Eq)]
enum View {
    Home,
    Music,
    Queue,
    QueueActions,
    QueueSong {
        id: u64,
        file: String,
        title: String,
    },
    ConfirmClear,
    NowPlaying,
    Artists,
    Artist(String),
    ArtistAlbums(String),
    ArtistSongs(String),
    Albums,
    AlbumSongs(String, Option<String>),
    Songs,
    Playlists,
    PlaylistSongs(String),
    SelectMusic(String),
    Directory(String),
    Actions(mpd::QueueSource, String),
    AddToPlaylist(String),
    Settings,
    Window,
    Playback,
    Volume,
    Crossfade,
    Repeat,
    Random,
    Future(&'static str),
    About,
}
impl View {
    fn title(&self) -> &str {
        match self {
            Self::Home => "",
            Self::AddToPlaylist(_) => "Add to Playlist",
            Self::SelectMusic(_) => "Select Music",
            Self::Settings => "Settings",
            Self::Window => "Window",
            Self::Playback => "Playback",
            Self::Volume => "Volume",
            Self::Crossfade => "Crossfade",
            Self::Repeat => "Repeat",
            Self::Random => "Shuffle",
            Self::Future(title) => title,
            Self::About => "About",
            Self::Music => "Music",
            Self::Queue => "Up Next",
            Self::QueueActions => "Queue Actions",
            Self::QueueSong { title, .. } => title,
            Self::ConfirmClear => "Clear Queue?",
            Self::NowPlaying => "Now Playing",
            Self::Artists => "Artists",
            Self::Albums => "Albums",
            Self::Songs => "Songs",
            Self::Playlists => "Playlists",
            Self::Artist(n)
            | Self::ArtistAlbums(n)
            | Self::ArtistSongs(n)
            | Self::AlbumSongs(n, _)
            | Self::PlaylistSongs(n)
            | Self::Actions(_, n) => n,
            Self::Directory(path) => {
                if path.is_empty() {
                    "Browse Files"
                } else {
                    friendly(path)
                }
            }
        }
    }
}
fn friendly(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[derive(Clone)]
enum Entry {
    Navigate(&'static str, View),
    ShuffleQueue,
    RandomisePlay,
    Queue(mpd::QueueSong),
    PlayQueue(u64),
    NextQueue(u64),
    Remove(u64),
    Artist(String),
    Album(String),
    Song(mpd::Song),
    Playlist(String),
    Folder(String),
    Action(&'static str, mpd::QueueAction),
    Clear,
    Cancel,
    PlaylistTarget(String, String),
    NewPlaylist,
    PickerSong(mpd::Song),
    CommitSelected,
    Setting(String, mpd::PlaybackSetting),
    Info(String),
    PinWindow,
    ResetWindow,
}
impl Entry {
    fn home_menu() -> Vec<Self> {
        vec![
            Self::Navigate("Music", View::Music),
            Self::RandomisePlay,
            Self::ShuffleQueue,
            Self::Navigate("Queue / Up Next", View::Queue),
            Self::Navigate("Now Playing", View::NowPlaying),
            Self::Navigate("Settings", View::Settings),
        ]
    }
    fn label(&self) -> &str {
        match self {
            Self::Navigate(label, _) | Self::Action(label, _) => label,
            Self::ShuffleQueue => "Shuffle Queue",
            Self::RandomisePlay => "Randomise Play",
            Self::PinWindow => "Always on Top",
            Self::ResetWindow => "Reset Window Size",
            Self::Clear => "Clear Queue",
            Self::Cancel => "Cancel",
            Self::Queue(s) => &s.song.title,
            Self::PlayQueue(_) => "Play Now",
            Self::NextQueue(_) => "Play Next",
            Self::Remove(_) => "Remove from Queue",
            Self::Artist(n) | Self::Album(n) | Self::Playlist(n) => n,
            Self::Folder(path) => friendly(path),
            Self::Song(s) | Self::PickerSong(s) => &s.title,
            Self::PlaylistTarget(name, _) | Self::Setting(name, _) | Self::Info(name) => name,
            Self::NewPlaylist => "New Playlist…",
            Self::CommitSelected => "Add Selected",
        }
    }
    fn display_label(&self, app: &AppWindow) -> String {
        use mpd::PlaybackSetting as S;
        match self {
            Self::PinWindow => format!(
                "Always on Top [{}]",
                if app.get_window_pinned() { "On" } else { "Off" }
            ),
            Self::Navigate(_, View::Volume) => format!(
                "Volume: {}",
                if app.get_player_volume() < 0 {
                    "Unavailable".into()
                } else {
                    format!("{}%", app.get_player_volume())
                }
            ),
            Self::Navigate(_, View::Crossfade) => format!(
                "Crossfade: {}",
                if app.get_player_crossfade() <= 0.0 {
                    "Off".into()
                } else {
                    format!("{}s", app.get_player_crossfade())
                }
            ),
            Self::Navigate(_, View::Repeat) => format!(
                "Repeat: {}",
                if app.get_player_repeat() { "On" } else { "Off" }
            ),
            Self::Navigate(_, View::Random) => format!(
                "Shuffle: {}",
                if app.get_player_random() { "On" } else { "Off" }
            ),
            Self::Setting(label, setting) => {
                let active = match *setting {
                    S::Volume(v) => app.get_player_volume() == i32::from(v),
                    S::Crossfade(v) => (app.get_player_crossfade() - f32::from(v)).abs() < 0.01,
                    S::Repeat(v) => app.get_player_repeat() == v,
                    S::Random(v) => app.get_player_random() == v,
                };
                format!("{}{}", if active { "● " } else { "" }, label)
            }
            _ => self.label().into(),
        }
    }
    fn is_leaf(&self) -> bool {
        matches!(
            self,
            Self::Song(_)
                | Self::Queue(_)
                | Self::Remove(_)
                | Self::PlayQueue(_)
                | Self::NextQueue(_)
                | Self::ShuffleQueue
                | Self::RandomisePlay
                | Self::Action(_, _)
                | Self::Clear
                | Self::Cancel
                | Self::PlaylistTarget(_, _)
                | Self::NewPlaylist
                | Self::PickerSong(_)
                | Self::CommitSelected
                | Self::Setting(_, _)
                | Self::Info(_)
                | Self::PinWindow
                | Self::ResetWindow
        )
    }
    fn is_shuffle(&self) -> bool {
        matches!(
            self,
            Self::ShuffleQueue | Self::RandomisePlay | Self::Action(_, mpd::QueueAction::Shuffle)
        )
    }
    fn is_song(&self) -> bool {
        matches!(self, Self::Song(_) | Self::PickerSong(_) | Self::Queue(_))
    }
    fn queue_position(&self) -> i32 {
        match self {
            Self::Queue(s) => i32::try_from(s.position).unwrap_or(-1),
            _ => -1,
        }
    }
}

struct Request {
    view: View,
    offset: usize,
    generation: u64,
    select_last: bool,
    select_index: Option<usize>,
}
enum Mutation {
    AddToPlaylist(String, String),
    CreatePlaylist(String, Option<String>),
    AddSelectedMusic {
        playlist: String,
        files: Vec<String>,
        added: usize,
    },
    Setting(mpd::PlaybackSetting),
    Source(mpd::QueueSource, mpd::QueueAction),
    PlayQueue(u64),
    NextQueue(u64),
    Remove(u64),
    Clear,
    ShuffleQueue,
    RandomisePlay,
}
impl Mutation {
    fn run(&mut self) -> Result<(), String> {
        match self {
            Self::Source(source, action) => mpd::queue_source(source, *action),
            Self::PlayQueue(id) => mpd::play_queue_id(*id),
            Self::NextQueue(id) => mpd::move_queue_next(*id),
            Self::Remove(id) => mpd::remove_queue_id(*id),
            Self::Clear => mpd::clear_queue(),
            Self::AddToPlaylist(name, file) => mpd::add_to_playlist(name, file),
            Self::CreatePlaylist(name, file) => mpd::create_playlist(name, file.as_deref()),
            Self::AddSelectedMusic {
                playlist,
                files,
                added,
            } => mpd::add_songs_to_playlist(playlist, files).map_err(|(count, err)| {
                *added = count;
                err
            }),
            Self::Setting(setting) => mpd::set_playback(*setting),
            Self::ShuffleQueue => mpd::shuffle_queue(),
            Self::RandomisePlay => mpd::shuffle_all_songs(),
        }
    }
    fn feedback(&self, succeeded: bool) -> Option<&'static str> {
        use mpd::{PlaybackSetting as S, QueueAction as A, QueueSource};
        if !succeeded {
            return matches!(self, Self::AddToPlaylist(_, _))
                .then_some("Unable to add to playlist");
        }
        match self {
            Self::AddToPlaylist(_, _) => Some("Added to playlist"),
            Self::CreatePlaylist(_, _) => Some("Playlist created"),
            Self::AddSelectedMusic { .. } => Some("Songs added to playlist"),
            Self::Source(_, A::Append) => Some("Added to queue"),
            Self::Source(_, A::PlayNext) => Some("Playing next"),
            Self::Source(QueueSource::Song(_), A::PlayNow) => Some("Added to queue"),
            Self::Source(_, A::PlayNow | A::Shuffle) | Self::RandomisePlay => {
                Some("Queue replaced")
            }
            Self::ShuffleQueue => Some("Queue shuffled"),
            Self::Remove(_) => Some("Removed from queue"),
            Self::NextQueue(_) => Some("Queue reordered"),
            Self::Clear => Some("Queue cleared"),
            Self::Setting(S::Repeat(true)) => Some("Repeat On"),
            Self::Setting(S::Repeat(false)) => Some("Repeat Off"),
            Self::Setting(S::Random(true)) => Some("Shuffle On"),
            Self::Setting(S::Random(false)) => Some("Shuffle Off"),
            Self::PlayQueue(_) | Self::Setting(_) => None,
        }
    }
    fn closes_action_menu(&self, view: &View) -> bool {
        matches!(view, View::Actions(_, _) | View::QueueSong { .. })
            && matches!(
                self,
                Self::Source(_, _)
                    | Self::PlayQueue(_)
                    | Self::NextQueue(_)
                    | Self::Remove(_)
                    | Self::AddToPlaylist(_, _)
            )
            || matches!(view, View::QueueActions) && matches!(self, Self::ShuffleQueue)
    }
    #[cfg(test)]
    fn starts_playback(&self) -> bool {
        matches!(
            self,
            Self::Source(_, mpd::QueueAction::PlayNow | mpd::QueueAction::Shuffle)
                | Self::RandomisePlay
        )
    }
}
enum Job {
    JumpLast(Request),
    FindPlaylist(Request, String),
    BulkSelect(u64, bool),
    PickerRange(u64, usize, usize, usize),
    Fetch(Request),
    Mutate(Mutation, u64),
}
struct Page {
    rows: Vec<Entry>,
    offset: usize,
    selected: usize,
    more: bool,
}
#[derive(Default)]
struct Browser {
    stack: Vec<View>,
    history: Vec<Page>,
    rows: Vec<Entry>,
    offset: usize,
    selected: usize,
    more: bool,
    loading: bool,
    generation: u64,
    // A mutation must complete before a queue refresh can replace its action page.
    mutating: bool,
    mutation_rows: Vec<Entry>,
    // None = not naming; Some(None) = empty playlist; Some(Some(file)) = create-and-add.
    playlist_entry_file: Option<Option<String>>,
    picker_files: std::collections::BTreeMap<usize, String>,
    picker_cache: std::collections::BTreeMap<usize, String>,
    picker_anchor: Option<usize>,
    picker_range_base: Option<std::collections::BTreeMap<usize, String>>,
    picker_pending_shift: Option<usize>,
}
impl Browser {
    fn view(&self) -> View {
        self.stack.last().cloned().unwrap_or(View::Home)
    }
    fn show(&self, app: &AppWindow, message: &str) {
        let title = self.view().title().to_string();
        app.set_volume_page(self.view() == View::Volume);
        app.set_picker_active(matches!(self.view(), View::SelectMusic(_)));
        app.set_page_title(title.into());
        app.set_browse_active(self.view() != View::NowPlaying);
        app.set_browse_message(message.into());
        app.set_browser_items(ModelRc::from(std::rc::Rc::new(VecModel::from(
            self.rows
                .iter()
                .enumerate()
                .map(|(i, r)| match r {
                    Entry::PickerSong(song) => SharedString::from(format!(
                        "{}{}",
                        if self.picker_files.get(&(self.offset + i - 1)) == Some(&song.file) {
                            "✓ "
                        } else {
                            "○ "
                        },
                        song.title
                    )),
                    Entry::CommitSelected => {
                        SharedString::from(format!("Add Selected ({})", self.picker_files.len()))
                    }
                    _ => SharedString::from(r.display_label(app)),
                })
                .collect::<Vec<_>>(),
        ))));
        app.set_browser_leaves(ModelRc::from(std::rc::Rc::new(VecModel::from(
            self.rows.iter().map(Entry::is_leaf).collect::<Vec<_>>(),
        ))));
        app.set_browser_shuffles(ModelRc::from(std::rc::Rc::new(VecModel::from(
            self.rows.iter().map(Entry::is_shuffle).collect::<Vec<_>>(),
        ))));
        app.set_browser_songs(ModelRc::from(std::rc::Rc::new(VecModel::from(
            self.rows.iter().map(Entry::is_song).collect::<Vec<_>>(),
        ))));
        app.set_browser_queue_positions(ModelRc::from(std::rc::Rc::new(VecModel::from(
            self.rows
                .iter()
                .map(Entry::queue_position)
                .collect::<Vec<_>>(),
        ))));
        app.set_selected_index(self.selected as i32);
    }
    fn fetch(&mut self, app: &AppWindow, tx: &mpsc::Sender<Job>, offset: usize, select_last: bool) {
        self.fetch_with_selection(app, tx, offset, select_last, None);
    }
    fn fetch_with_selection(
        &mut self,
        app: &AppWindow,
        tx: &mpsc::Sender<Job>,
        offset: usize,
        select_last: bool,
        select_index: Option<usize>,
    ) {
        self.offset = offset;
        self.selected = 0;
        self.rows.clear();
        self.more = false;
        self.loading = true;
        self.generation += 1;
        let request = Request {
            view: self.view(),
            offset,
            generation: self.generation,
            select_last,
            select_index,
        };
        self.show(app, "Loading…");
        if tx.send(Job::Fetch(request)).is_err() {
            self.loading = false;
            self.show(app, "Browser worker stopped");
        }
    }
    fn find_created_playlist(&mut self, name: String, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        self.loading = true;
        self.rows.clear();
        self.generation += 1;
        self.show(app, "Loading…");
        let request = Request {
            view: self.view(),
            offset: 0,
            generation: self.generation,
            select_last: false,
            select_index: None,
        };
        if tx.send(Job::FindPlaylist(request, name)).is_err() {
            self.loading = false;
            self.show(app, "Browser worker stopped");
        }
    }
    fn push(&mut self, view: View, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if matches!(view, View::SelectMusic(_)) {
            self.picker_files.clear();
            self.picker_cache.clear();
            self.picker_anchor = None;
            self.picker_range_base = None;
            self.picker_pending_shift = None;
        }
        self.history.push(Page {
            rows: self.rows.clone(),
            offset: self.offset,
            selected: self.selected,
            more: self.more,
        });
        self.stack.push(view);
        if self.view() == View::NowPlaying {
            self.generation += 1;
            self.rows.clear();
            self.loading = false;
            self.selected = 0;
            self.show(app, "");
        } else if self.view() == View::Volume {
            // The slider is an interactive page, not a paginated set of 101 actions.
            self.generation += 1;
            self.rows.clear();
            self.loading = false;
            self.selected = 0;
            self.show(app, "");
        } else {
            self.fetch(app, tx, 0, false);
        }
    }
    fn toggle_picker(&mut self, row: usize, file: String, app: &AppWindow) {
        let index = self.offset + row - 1;
        if self.picker_files.remove(&index).is_none() {
            self.picker_files.insert(index, file);
        }
        self.picker_anchor = Some(index);
        self.picker_range_base = None;
        self.show(app, "");
    }
    fn picker_space(&mut self, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if self.loading || self.mutating {
            return;
        }
        let row = app.get_selected_index().max(0) as usize;
        if let Some(Entry::PickerSong(song)) = self.rows.get(row) {
            self.selected = row;
            self.toggle_picker(row, song.file.clone(), app);
            self.step(1, app, tx);
        }
    }
    fn picker_bulk(&mut self, invert: bool, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if !matches!(self.view(), View::SelectMusic(_)) || self.loading || self.mutating {
            return;
        }
        self.loading = true;
        self.generation += 1;
        app.invoke_show_status("Scanning library…".into());
        if tx.send(Job::BulkSelect(self.generation, invert)).is_err() {
            self.loading = false;
            app.invoke_show_status("Browser worker stopped".into());
        }
    }
    fn complete_bulk(
        &mut self,
        app: &AppWindow,
        generation: u64,
        invert: bool,
        result: Result<Vec<String>, String>,
    ) {
        if generation != self.generation || !matches!(self.view(), View::SelectMusic(_)) {
            return;
        }
        self.loading = false;
        match result {
            Ok(files) => {
                if !invert {
                    self.picker_files.clear();
                } else {
                    self.picker_files.retain(|index, _| *index < files.len());
                }
                for (index, file) in files.into_iter().enumerate() {
                    if invert && self.picker_files.remove(&index).is_some() {
                        continue;
                    }
                    self.picker_files.insert(index, file);
                }
                self.picker_range_base = None;
                self.picker_anchor = None;
                app.invoke_show_status(
                    format!("{} songs selected", self.picker_files.len()).into(),
                );
                self.show(app, "");
            }
            Err(err) => {
                eprintln!("MPD song selection: {err}");
                app.invoke_show_status("Could not scan library".into());
            }
        }
    }
    fn apply_picker_range(&mut self, end: usize, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        let anchor = self.picker_anchor.unwrap_or(end);
        let start = anchor.min(end);
        let stop = anchor.max(end);
        if (start..=stop).any(|index| !self.picker_cache.contains_key(&index)) {
            self.loading = true;
            self.generation += 1;
            if tx
                .send(Job::PickerRange(self.generation, start, stop, end))
                .is_err()
            {
                self.loading = false;
                app.invoke_show_status("Browser worker stopped".into());
            }
            return;
        }
        let base = self
            .picker_range_base
            .get_or_insert_with(|| self.picker_files.clone())
            .clone();
        self.picker_files = base;
        for index in start..=stop {
            self.picker_files
                .insert(index, self.picker_cache[&index].clone());
        }
        self.show(app, "");
    }
    fn complete_picker_range(
        &mut self,
        app: &AppWindow,
        tx: &mpsc::Sender<Job>,
        generation: u64,
        end: usize,
        result: Result<Vec<(usize, String)>, String>,
    ) {
        if generation != self.generation || !matches!(self.view(), View::SelectMusic(_)) {
            return;
        }
        self.loading = false;
        match result {
            Ok(files) => {
                self.picker_cache.extend(files);
                self.apply_picker_range(end, app, tx);
            }
            Err(err) => {
                eprintln!("MPD song range: {err}");
                self.picker_range_base = None;
                app.invoke_show_status("Could not select range".into());
            }
        }
    }
    fn shift_picker(&mut self, delta: i32, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if !matches!(self.view(), View::SelectMusic(_))
            || self.loading
            || self.mutating
            || delta == 0
        {
            return;
        }
        self.selected =
            (app.get_selected_index().max(0) as usize).min(self.rows.len().saturating_sub(1));
        let Some(Entry::PickerSong(_)) = self.rows.get(self.selected) else {
            return;
        };
        let current = self.offset + self.selected - 1;
        let Some(end) = current.checked_add_signed(delta.signum() as isize) else {
            return;
        };
        if self.picker_anchor.is_none() {
            self.picker_anchor = Some(current);
        }
        if end >= self.offset.saturating_sub(1) && end < self.offset + self.rows.len() - 1 {
            let row = end + 1 - self.offset;
            if matches!(self.rows.get(row), Some(Entry::PickerSong(_))) {
                self.selected = row;
                self.apply_picker_range(end, app, tx);
            }
        } else if end < current && self.offset >= PAGE {
            self.picker_pending_shift = Some(end);
            self.fetch_with_selection(app, tx, self.offset - PAGE, false, Some(PAGE - 1));
        } else if end > current && self.more {
            self.picker_pending_shift = Some(end);
            self.fetch_with_selection(app, tx, self.offset + PAGE, false, Some(0));
        }
    }
    fn mutate(&mut self, mutation: Mutation, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        self.loading = true;
        self.mutating = true;
        self.generation += 1;
        self.mutation_rows = std::mem::take(&mut self.rows);
        self.show(app, "Working…");
        if tx.send(Job::Mutate(mutation, self.generation)).is_err() {
            self.loading = false;
            self.mutating = false;
            self.show(app, "Browser worker stopped — MENU to go back");
        }
    }
    fn submit_playlist_name(&mut self, name: &str, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if self.mutating || self.playlist_entry_file.is_none() {
            return;
        }
        match mpd::playlist_name(name) {
            Ok(name) => {
                app.set_playlist_entry_error("Creating…".into());
                self.mutate(
                    Mutation::CreatePlaylist(
                        name.into(),
                        self.playlist_entry_file.clone().unwrap(),
                    ),
                    app,
                    tx,
                );
            }
            Err(err) => app.set_playlist_entry_error(err.into()),
        }
    }
    fn cancel_playlist_name(&mut self, app: &AppWindow) {
        if self.mutating {
            return;
        }
        self.playlist_entry_file = None;
        app.set_playlist_entry_open(false);
        app.set_playlist_entry_error("".into());
    }
    fn open(&mut self, index: i32, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if self.loading || self.playlist_entry_file.is_some() || index < 0 {
            return;
        }
        let Some(entry) = self.rows.get(index as usize).cloned() else {
            return;
        };
        self.selected = index as usize;
        match entry {
            Entry::Navigate(_, view) => self.push(view, app, tx),
            Entry::ShuffleQueue => self.mutate(Mutation::ShuffleQueue, app, tx),
            Entry::RandomisePlay => self.mutate(Mutation::RandomisePlay, app, tx),
            Entry::Queue(song) => self.push(
                View::QueueSong {
                    id: song.id,
                    file: song.song.file,
                    title: song.song.title,
                },
                app,
                tx,
            ),
            Entry::PlayQueue(id) => self.mutate(Mutation::PlayQueue(id), app, tx),
            Entry::NextQueue(id) => self.mutate(Mutation::NextQueue(id), app, tx),
            Entry::Remove(id) => self.mutate(Mutation::Remove(id), app, tx),
            Entry::Clear => self.mutate(Mutation::Clear, app, tx),
            Entry::Cancel => self.back(app, tx),
            Entry::PlaylistTarget(name, file) => {
                self.mutate(Mutation::AddToPlaylist(name, file), app, tx)
            }
            Entry::PickerSong(song) => self.toggle_picker(index as usize, song.file, app),
            Entry::CommitSelected => {
                if let View::SelectMusic(playlist) = self.view() {
                    if self.picker_files.is_empty() {
                        app.invoke_show_status("Select songs first".into());
                    } else {
                        let files = self.picker_files.values().cloned().collect();
                        self.mutate(
                            Mutation::AddSelectedMusic {
                                playlist,
                                files,
                                added: 0,
                            },
                            app,
                            tx,
                        );
                    }
                }
            }
            Entry::NewPlaylist => {
                let file = match self.view() {
                    View::AddToPlaylist(file) => Some(file),
                    _ => None,
                };
                self.playlist_entry_file = Some(file);
                app.set_playlist_entry_text("".into());
                app.set_playlist_entry_error("".into());
                app.set_playlist_entry_open(true);
            }
            Entry::Setting(_, setting) => self.mutate(Mutation::Setting(setting), app, tx),
            Entry::Info(_) => {}
            Entry::PinWindow => {
                app.invoke_window_pin_requested();
                self.show(app, "");
            }
            Entry::ResetWindow => app.invoke_window_reset_requested(),
            Entry::Artist(name) => self.push(
                View::Actions(mpd::QueueSource::Artist(name.clone()), name),
                app,
                tx,
            ),
            Entry::Folder(path) => {
                let title = friendly(&path).to_string();
                self.push(
                    View::Actions(mpd::QueueSource::Folder(path), title),
                    app,
                    tx,
                );
            }
            Entry::Album(name) => {
                let artist = if let View::ArtistAlbums(name) = self.view() {
                    Some(name)
                } else {
                    None
                };
                self.push(
                    View::Actions(mpd::QueueSource::Album(name.clone(), artist), name),
                    app,
                    tx,
                );
            }
            Entry::Playlist(name) => self.push(
                View::Actions(mpd::QueueSource::Playlist(name.clone()), name),
                app,
                tx,
            ),
            Entry::Song(song) => self.push(
                View::Actions(mpd::QueueSource::Song(song.file), song.title),
                app,
                tx,
            ),
            Entry::Action(_, action) => {
                let source = match self.view() {
                    View::Actions(source, _) => Some(source),
                    View::QueueSong { file, .. } => Some(mpd::QueueSource::Song(file)),
                    _ => None,
                };
                if let Some(source) = source {
                    self.mutate(Mutation::Source(source, action), app, tx);
                }
            }
        }
    }
    fn back(&mut self, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if self.playlist_entry_file.is_some() {
            self.cancel_playlist_name(app);
            return;
        }
        // Avoid executing a second action while the first is still in flight.
        if self.mutating {
            return;
        }
        if matches!(self.view(), View::SelectMusic(_)) {
            self.picker_files.clear();
            self.picker_cache.clear();
            self.picker_anchor = None;
            self.picker_range_base = None;
            self.picker_pending_shift = None;
        }
        if self.stack.pop().is_none() {
            if self.rows.is_empty() {
                self.fetch(app, tx, 0, false);
            }
            return;
        }
        self.generation += 1;
        self.loading = false;
        if let Some(page) = self.history.pop() {
            self.rows = page.rows;
            self.offset = page.offset;
            self.selected = page.selected;
            self.more = page.more;
        }
        // Cached queue rows may have changed in rmpc while a child page was open.
        if matches!(
            self.view(),
            View::Queue | View::PlaylistSongs(_) | View::AddToPlaylist(_)
        ) {
            self.fetch_with_selection(app, tx, self.offset, false, Some(self.selected));
        } else {
            self.show(app, "");
        }
    }
    fn jump(&mut self, last: bool, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if self.loading || self.playlist_entry_file.is_some() || self.view() == View::NowPlaying {
            return;
        }
        if !last {
            self.fetch(app, tx, 0, false);
            return;
        }
        self.generation += 1;
        self.loading = true;
        self.rows.clear();
        self.show(app, "Loading…");
        let request = Request {
            view: self.view(),
            offset: 0,
            generation: self.generation,
            select_last: true,
            select_index: None,
        };
        if tx.send(Job::JumpLast(request)).is_err() {
            self.loading = false;
            self.show(app, "Browser worker stopped");
        }
    }

    fn step(&mut self, delta: i32, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if self.loading || self.playlist_entry_file.is_some() || self.rows.is_empty() {
            return;
        }
        // Mouse selection and wheel selection must share the same index.
        if matches!(self.view(), View::SelectMusic(_)) && self.picker_range_base.is_some() {
            self.picker_range_base = None;
            self.picker_anchor = None;
        }
        self.selected = (app.get_selected_index().max(0) as usize).min(self.rows.len() - 1);
        if delta > 0 {
            if self.selected + 1 < self.rows.len() {
                self.selected += 1;
            } else if self.more {
                self.fetch(app, tx, self.offset + PAGE, false);
                return;
            }
        } else if delta < 0 {
            if self.selected > 0 {
                self.selected -= 1;
            } else if self.offset >= PAGE {
                self.fetch(app, tx, self.offset - PAGE, true);
                return;
            }
        }
        self.show(app, "");
    }
    fn complete(
        &mut self,
        app: &AppWindow,
        tx: &mpsc::Sender<Job>,
        request: Request,
        result: Result<Vec<Entry>, String>,
    ) {
        if request.generation != self.generation || request.view != self.view() {
            return;
        }
        self.loading = false;
        match result {
            Ok(mut rows) => {
                // Removing the last row of a page (also externally) returns to a valid page.
                if rows.is_empty() && request.offset > 0 {
                    self.fetch(app, tx, request.offset.saturating_sub(PAGE), true);
                    return;
                }
                self.offset = request.offset;
                self.more = rows.len() > PAGE;
                rows.truncate(PAGE);
                self.rows = rows;
                if matches!(request.view, View::SelectMusic(_)) {
                    for (i, entry) in self.rows.iter().enumerate() {
                        if let Entry::PickerSong(song) = entry {
                            self.picker_cache
                                .insert(request.offset + i - 1, song.file.clone());
                        }
                    }
                }
                self.selected = if request.select_last {
                    self.rows.len().saturating_sub(1)
                } else {
                    request
                        .select_index
                        .unwrap_or(0)
                        .min(self.rows.len().saturating_sub(1))
                };
                if let Some(end) = self.picker_pending_shift.take() {
                    self.apply_picker_range(end, app, tx);
                }
                self.show(
                    app,
                    if self.rows.is_empty() {
                        "No items found"
                    } else {
                        ""
                    },
                );
            }
            Err(err) => {
                eprintln!("MPD library: {err}");
                self.picker_pending_shift = None;
                self.show(app, "Could not load — MENU to go back");
            }
        }
    }
    fn finish(
        &mut self,
        app: &AppWindow,
        tx: &mpsc::Sender<Job>,
        generation: u64,
        mutation: Mutation,
        result: Result<(), String>,
    ) {
        if generation != self.generation {
            return;
        }
        self.loading = false;
        self.mutating = false;
        if let Some(message) = mutation.feedback(result.is_ok()) {
            app.invoke_show_status(message.into());
        }
        match result {
            Ok(()) => {
                self.rows = std::mem::take(&mut self.mutation_rows);
                match mutation {
                    Mutation::AddSelectedMusic { playlist, .. } => {
                        self.back(app, tx);
                        self.push(View::PlaylistSongs(playlist), app, tx);
                    }
                    Mutation::CreatePlaylist(name, file) => {
                        self.playlist_entry_file = None;
                        app.set_playlist_entry_open(false);
                        app.set_playlist_entry_error("".into());
                        if file.is_some() {
                            self.leave_action_flow(app, tx);
                        } else {
                            self.find_created_playlist(name, app, tx);
                        }
                    }
                    Mutation::AddToPlaylist(_, _) => self.leave_action_flow(app, tx),
                    Mutation::Clear => {
                        while self.view() != View::Queue && !self.stack.is_empty() {
                            self.stack.pop();
                            self.history.pop();
                        }
                        self.fetch(app, tx, 0, false);
                    }
                    Mutation::RandomisePlay => self.push(View::NowPlaying, app, tx),
                    _ if mutation.closes_action_menu(&self.view()) => self.back(app, tx),
                    _ => self.fetch(app, tx, self.offset, false),
                }
            }
            Err(err) => {
                eprintln!("MPD queue action: {err}");
                self.rows = std::mem::take(&mut self.mutation_rows);
                if let Mutation::AddSelectedMusic { files, added, .. } = &mutation {
                    for file in files.iter().take(*added) {
                        self.picker_files.retain(|_, selected| selected != file);
                    }
                    self.picker_range_base = None;
                    app.invoke_show_status(if *added > 0 {
                        format!("{added} added; {} still selected", self.picker_files.len()).into()
                    } else {
                        err.clone().into()
                    });
                    self.show(app, "");
                    return;
                }
                if matches!(mutation, Mutation::CreatePlaylist(_, _)) {
                    app.set_playlist_entry_error(err.into());
                } else {
                    app.invoke_show_status("Action failed".into());
                }
                self.show(app, "");
            }
        }
    }
    fn leave_action_flow(&mut self, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if matches!(self.view(), View::AddToPlaylist(_)) {
            self.back(app, tx);
        }
        if matches!(self.view(), View::Actions(_, _) | View::QueueSong { .. }) {
            self.back(app, tx);
        }
    }
    fn refresh_queue(&mut self, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if !self.mutating && matches!(self.view(), View::Queue) {
            self.fetch(app, tx, self.offset, false);
        }
    }
}

// Exponential search then binary search: logarithmic page requests, no UI blocking
// and no linear walk through a large library. Fetch already bounds each song page.
fn last_page<T>(
    mut fetch: impl FnMut(usize) -> Result<Vec<T>, String>,
) -> Result<(usize, Vec<T>), String> {
    let first = fetch(0)?;
    if first.len() <= PAGE {
        return Ok((0, first));
    }
    let mut lo = 0usize;
    let mut hi = 1usize;
    while !fetch(hi * PAGE)?.is_empty() {
        lo = hi;
        hi = hi
            .checked_mul(2)
            .filter(|n| *n <= usize::MAX / PAGE)
            .ok_or("List too large")?;
    }
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if fetch(mid * PAGE)?.is_empty() {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Ok((lo * PAGE, fetch(lo * PAGE)?))
}

fn fetch_last(view: &View) -> Result<(usize, Vec<Entry>), String> {
    use mpd::QueueSource as S;
    let count = match view {
        View::Queue => Some(mpd::queue_length()?.saturating_add(1)),
        View::Songs => Some(mpd::library_length()?),
        View::ArtistSongs(name) => Some(mpd::source_length(&S::Artist(name.clone()))?),
        View::AlbumSongs(name, artist) => {
            Some(mpd::source_length(&S::Album(name.clone(), artist.clone()))?)
        }
        View::PlaylistSongs(name) => Some(mpd::source_length(&S::Playlist(name.clone()))?),
        _ => None,
    };
    if let Some(count) = count {
        let offset = count.saturating_sub(1) / PAGE * PAGE;
        return Ok((offset, fetch(view, offset)?));
    }
    last_page(|offset| fetch(view, offset))
}

fn queue_window(offset: usize) -> (usize, usize) {
    // The first visible row is Queue Actions, not MPD position zero.
    (
        offset.saturating_sub(1),
        PAGE + 1 - usize::from(offset == 0),
    )
}

fn page(rows: Vec<Entry>, offset: usize) -> Vec<Entry> {
    rows.into_iter().skip(offset).take(PAGE + 1).collect()
}
fn fetch(view: &View, offset: usize) -> Result<Vec<Entry>, String> {
    use mpd::{QueueAction as A, QueueSource as S};
    let limit = PAGE + 1;
    Ok(match view {
        View::Home => page(Entry::home_menu(), offset),
        View::Settings => page(
            vec![
                Entry::Navigate("Playback", View::Playback),
                #[cfg(target_os = "macos")]
                Entry::Navigate("Window", View::Window),
                Entry::Navigate("Controls (future)", View::Future("Controls")),
                Entry::Navigate("Appearance (future)", View::Future("Appearance")),
                Entry::Navigate("About", View::About),
            ],
            offset,
        ),
        View::Playback => page(
            vec![
                Entry::Navigate("Volume", View::Volume),
                Entry::Navigate("Crossfade", View::Crossfade),
                Entry::Navigate("Repeat", View::Repeat),
                Entry::Navigate("Shuffle", View::Random),
            ],
            offset,
        ),
        View::Window => page(vec![Entry::PinWindow, Entry::ResetWindow], offset),
        View::Volume => Vec::new(),
        View::Crossfade => page(
            [0, 1, 2, 3, 5, 10]
                .into_iter()
                .map(|v| {
                    Entry::Setting(
                        if v == 0 {
                            "Off".into()
                        } else {
                            format!("{v} seconds")
                        },
                        mpd::PlaybackSetting::Crossfade(v),
                    )
                })
                .collect(),
            offset,
        ),
        View::Repeat | View::Random => page(
            [false, true]
                .into_iter()
                .map(|v| {
                    Entry::Setting(
                        if v { "On".into() } else { "Off".into() },
                        if *view == View::Repeat {
                            mpd::PlaybackSetting::Repeat(v)
                        } else {
                            mpd::PlaybackSetting::Random(v)
                        },
                    )
                })
                .collect(),
            offset,
        ),
        View::Future(_) => vec![Entry::Info("Coming later".into())],
        View::About => vec![
            Entry::Info(format!("iPod Player {}", env!("CARGO_PKG_VERSION"))),
            Entry::Info("Rust + Slint / MPD".into()),
        ],
        View::AddToPlaylist(file) => {
            let mut rows = vec![Entry::NewPlaylist];
            rows.extend(
                mpd::list_playlists()?
                    .into_iter()
                    .map(|p| Entry::PlaylistTarget(p.name, file.clone())),
            );
            page(rows, offset)
        }
        View::Music => page(
            vec![
                Entry::Navigate("Playlists", View::Playlists),
                Entry::Navigate("Artists", View::Artists),
                Entry::Navigate("Albums", View::Albums),
                Entry::Navigate("Songs", View::Songs),
                Entry::Navigate("Browse Files", View::Directory(String::new())),
            ],
            offset,
        ),
        View::Artist(name) => page(
            vec![
                Entry::Navigate("Albums", View::ArtistAlbums(name.clone())),
                Entry::Navigate("Songs", View::ArtistSongs(name.clone())),
            ],
            offset,
        ),
        View::QueueActions => page(
            vec![
                Entry::ShuffleQueue,
                Entry::Navigate("Clear Queue…", View::ConfirmClear),
            ],
            offset,
        ),
        View::ConfirmClear => page(vec![Entry::Cancel, Entry::Clear], offset),
        View::Actions(source, _) => {
            let rows = match source {
                S::Song(file) => vec![
                    Entry::Action("Play Now", A::PlayNow),
                    Entry::Action("Play Next", A::PlayNext),
                    Entry::Action("Add to Queue", A::Append),
                    Entry::Navigate("Add to Playlist", View::AddToPlaylist(file.clone())),
                ],
                S::Album(name, artist) => vec![
                    Entry::Navigate(
                        "Browse Songs",
                        View::AlbumSongs(name.clone(), artist.clone()),
                    ),
                    Entry::Action("Play Album", A::PlayNow),
                    Entry::Action("Shuffle Album", A::Shuffle),
                    Entry::Action("Play Next", A::PlayNext),
                    Entry::Action("Add Album to Queue", A::Append),
                ],
                S::Playlist(name) => vec![
                    Entry::Navigate("Browse Songs", View::PlaylistSongs(name.clone())),
                    Entry::Navigate("Select Music", View::SelectMusic(name.clone())),
                    Entry::Action("Play Playlist", A::PlayNow),
                    Entry::Action("Shuffle Playlist", A::Shuffle),
                    Entry::Action("Play Next", A::PlayNext),
                    Entry::Action("Add Playlist to Queue", A::Append),
                ],
                S::Artist(name) => vec![
                    Entry::Navigate("Browse Artist", View::Artist(name.clone())),
                    Entry::Action("Play Artist", A::PlayNow),
                    Entry::Action("Shuffle Artist", A::Shuffle),
                    Entry::Action("Play Next", A::PlayNext),
                    Entry::Action("Add to Queue", A::Append),
                ],
                S::Folder(path) => vec![
                    Entry::Navigate("Browse Folder", View::Directory(path.clone())),
                    Entry::Action("Play Folder", A::PlayNow),
                    Entry::Action("Shuffle Folder", A::Shuffle),
                    Entry::Action("Play Next", A::PlayNext),
                    Entry::Action("Add to Queue", A::Append),
                ],
            };
            page(rows, offset)
        }
        View::Artists => page(
            mpd::list_artists()?
                .into_iter()
                .map(|a| Entry::Artist(a.name))
                .collect(),
            offset,
        ),
        View::Albums => page(
            mpd::list_albums()?
                .into_iter()
                .map(|a| Entry::Album(a.name))
                .collect(),
            offset,
        ),
        View::ArtistAlbums(name) => page(
            mpd::albums_by_artist(name)?
                .into_iter()
                .map(|a| Entry::Album(a.name))
                .collect(),
            offset,
        ),
        View::Playlists => {
            let mut rows = vec![Entry::NewPlaylist];
            rows.extend(
                mpd::list_playlists()?
                    .into_iter()
                    .map(|p| Entry::Playlist(p.name)),
            );
            page(rows, offset)
        }
        View::SelectMusic(_) => {
            let mut rows = Vec::new();
            if offset == 0 {
                rows.push(Entry::CommitSelected);
            }
            rows.extend(
                mpd::list_songs(offset.saturating_sub(1), PAGE + 1 - rows.len())?
                    .into_iter()
                    .map(Entry::PickerSong),
            );
            rows
        }
        View::Songs => mpd::list_songs(offset, limit)?
            .into_iter()
            .map(Entry::Song)
            .collect(),
        View::ArtistSongs(name) => mpd::songs_by_artist(name, offset, limit)?
            .into_iter()
            .map(Entry::Song)
            .collect(),
        View::AlbumSongs(name, Some(artist)) => {
            mpd::songs_by_album_and_artist(name, artist, offset, limit)?
                .into_iter()
                .map(Entry::Song)
                .collect()
        }
        View::AlbumSongs(name, None) => mpd::songs_by_album(name, offset, limit)?
            .into_iter()
            .map(Entry::Song)
            .collect(),
        View::PlaylistSongs(name) => mpd::songs_by_playlist(name, offset, limit)?
            .into_iter()
            .map(Entry::Song)
            .collect(),
        View::Directory(path) => page(
            mpd::browse_directory(path)?
                .into_iter()
                .map(|e| match e {
                    mpd::DirectoryEntry::Folder(path) => Entry::Folder(path),
                    mpd::DirectoryEntry::Song(song) => Entry::Song(song),
                })
                .collect(),
            offset,
        ),
        View::Queue => {
            // One virtual header row, with the remainder paged by actual MPD positions.
            let mut rows = Vec::new();
            if offset == 0 {
                rows.push(Entry::Navigate("Queue Actions", View::QueueActions));
            }
            let (position, count) = queue_window(offset);
            rows.extend(
                mpd::queue_songs(position, count)?
                    .into_iter()
                    .map(Entry::Queue),
            );
            rows
        }
        View::QueueSong { id, file, .. } => page(
            vec![
                Entry::PlayQueue(*id),
                Entry::NextQueue(*id),
                Entry::Action("Add to Queue", A::Append),
                Entry::Navigate("Add to Playlist", View::AddToPlaylist(file.clone())),
                Entry::Remove(*id),
            ],
            offset,
        ),
        View::NowPlaying => Vec::new(),
    })
}

pub fn install(app: &AppWindow, status_sender: mpsc::Sender<StatusCommand>) {
    let (tx, rx) = mpsc::channel::<Job>();
    let browser = Arc::new(Mutex::new(Browser::default()));
    let weak = app.as_weak();
    let controller = browser.clone();
    let worker_tx = tx.clone();
    thread::spawn(move || {
        while let Ok(job) = rx.recv() {
            let weak = weak.clone();
            let controller = controller.clone();
            let tx = worker_tx.clone();
            let scheduled = match job {
                Job::JumpLast(mut request) => {
                    let result = fetch_last(&request.view);
                    let result = result.map(|(offset, rows)| {
                        request.offset = offset;
                        rows
                    });
                    slint::invoke_from_event_loop(move || {
                        if let Some(app) = weak.upgrade() {
                            controller
                                .lock()
                                .unwrap()
                                .complete(&app, &tx, request, result);
                        }
                    })
                }
                Job::FindPlaylist(mut request, name) => {
                    let result = mpd::list_playlists().and_then(|playlists| {
                        let index = playlists
                            .iter()
                            .position(|p| p.name == name)
                            .ok_or("New playlist not found after creation")?
                            + 1;
                        request.offset = index / PAGE * PAGE;
                        request.select_index = Some(index % PAGE);
                        fetch(&request.view, request.offset)
                    });
                    slint::invoke_from_event_loop(move || {
                        if let Some(app) = weak.upgrade() {
                            controller
                                .lock()
                                .unwrap()
                                .complete(&app, &tx, request, result);
                        }
                    })
                }
                Job::PickerRange(generation, start, stop, end) => {
                    let result = mpd::song_files_in_range(start, stop);
                    slint::invoke_from_event_loop(move || {
                        if let Some(app) = weak.upgrade() {
                            controller
                                .lock()
                                .unwrap()
                                .complete_picker_range(&app, &tx, generation, end, result);
                        }
                    })
                }
                Job::BulkSelect(generation, invert) => {
                    let result = mpd::all_song_files();
                    slint::invoke_from_event_loop(move || {
                        if let Some(app) = weak.upgrade() {
                            controller
                                .lock()
                                .unwrap()
                                .complete_bulk(&app, generation, invert, result);
                        }
                    })
                }
                Job::Fetch(request) => {
                    let result = fetch(&request.view, request.offset);
                    slint::invoke_from_event_loop(move || {
                        if let Some(app) = weak.upgrade() {
                            controller
                                .lock()
                                .unwrap()
                                .complete(&app, &tx, request, result);
                        }
                    })
                }
                Job::Mutate(mut mutation, generation) => {
                    let result = mutation.run();
                    // Refresh even on partial command-list failure; never pretend it rolled back.
                    let _ = status_sender.send(StatusCommand::Refresh);
                    slint::invoke_from_event_loop(move || {
                        if let Some(app) = weak.upgrade() {
                            controller
                                .lock()
                                .unwrap()
                                .finish(&app, &tx, generation, mutation, result);
                        }
                    })
                }
            };
            if scheduled.is_err() {
                break;
            }
        }
    });
    browser.lock().unwrap().fetch(app, &tx, 0, false);
    let settings_controller = browser.clone();
    let settings_weak = app.as_weak();
    app.on_refresh_settings(move || {
        if let Some(app) = settings_weak.upgrade() {
            let mut controller = settings_controller.lock().unwrap();
            if !controller.loading
                && matches!(
                    controller.view(),
                    View::Playback | View::Volume | View::Crossfade | View::Repeat | View::Random
                )
            {
                // Status updates must not reset a mouse-selected settings row.
                controller.selected = (app.get_selected_index().max(0) as usize)
                    .min(controller.rows.len().saturating_sub(1));
                controller.show(&app, "");
            }
        }
    });
    let controller = browser.clone();
    let weak = app.as_weak();
    let requests = tx.clone();
    app.on_browse_jump(move |last| {
        if let Some(app) = weak.upgrade() {
            controller.lock().unwrap().jump(last, &app, &requests);
        }
    });
    let controller = browser.clone();
    let weak = app.as_weak();
    let requests = tx.clone();
    app.on_browse_open(move |index| {
        if let Some(app) = weak.upgrade() {
            controller.lock().unwrap().open(index, &app, &requests);
        }
    });
    let controller = browser.clone();
    let weak = app.as_weak();
    let requests = tx.clone();
    app.on_browse_pick_space(move || {
        if let Some(app) = weak.upgrade() {
            controller.lock().unwrap().picker_space(&app, &requests);
        }
    });
    let controller = browser.clone();
    let weak = app.as_weak();
    let requests = tx.clone();
    app.on_browse_shift_step(move |delta| {
        if let Some(app) = weak.upgrade() {
            controller
                .lock()
                .unwrap()
                .shift_picker(delta, &app, &requests);
        }
    });
    let controller = browser.clone();
    let weak = app.as_weak();
    let requests = tx.clone();
    app.on_browse_bulk_select(move |invert| {
        if let Some(app) = weak.upgrade() {
            controller
                .lock()
                .unwrap()
                .picker_bulk(invert, &app, &requests);
        }
    });
    let controller = browser.clone();
    let weak = app.as_weak();
    let requests = tx.clone();
    app.on_browse_back(move || {
        if let Some(app) = weak.upgrade() {
            controller.lock().unwrap().back(&app, &requests);
        }
    });
    let controller = browser.clone();
    let weak = app.as_weak();
    let requests = tx.clone();
    app.on_refresh_queue(move || {
        if let Some(app) = weak.upgrade() {
            controller.lock().unwrap().refresh_queue(&app, &requests);
        }
    });
    let controller = browser.clone();
    let weak = app.as_weak();
    let requests = tx.clone();
    app.on_playlist_name_submitted(move |name| {
        if let Some(app) = weak.upgrade() {
            controller
                .lock()
                .unwrap()
                .submit_playlist_name(&name, &app, &requests);
        }
    });
    let controller = browser.clone();
    let weak = app.as_weak();
    app.on_playlist_name_cancelled(move || {
        if let Some(app) = weak.upgrade() {
            controller.lock().unwrap().cancel_playlist_name(&app);
        }
    });
    let weak = app.as_weak();
    app.on_browse_step(move |delta| {
        if let Some(app) = weak.upgrade() {
            browser.lock().unwrap().step(delta, &app, &tx);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_navigation_preserves_pagination_and_never_activates() {
        use slint::platform::{
            Platform, PlatformError, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(
                &self,
            ) -> Result<std::rc::Rc<dyn WindowAdapter>, PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let app = AppWindow::new().unwrap();
        for count in [0usize, 1, 5, 6, 7, 12, 13] {
            let (tx, rx) = mpsc::channel();
            let mut browser = Browser::default();
            let rows = |offset| {
                (offset..count)
                    .take(PAGE + 1)
                    .map(|i| Entry::Info(i.to_string()))
                    .collect::<Vec<_>>()
            };
            let complete = |browser: &mut Browser| {
                while let Ok(job) = rx.try_recv() {
                    let (request, entries) = match job {
                        Job::Fetch(request) => {
                            let entries = rows(request.offset);
                            (request, entries)
                        }
                        Job::FindPlaylist(_, _) => panic!("No saved playlists in this test"),
                        Job::BulkSelect(_, _) | Job::PickerRange(_, _, _, _) => {
                            panic!("No selection in this test")
                        }
                        Job::JumpLast(mut request) => {
                            let (offset, entries) = last_page(|offset| Ok(rows(offset))).unwrap();
                            request.offset = offset;
                            (request, entries)
                        }
                        Job::Mutate(_, _) => panic!("navigation activated an item"),
                    };
                    browser.complete(&app, &tx, request, Ok(entries));
                }
            };
            browser.fetch(&app, &tx, 0, false);
            complete(&mut browser);
            let mut expected = 0usize;
            for delta in
                std::iter::repeat_n(-1, count + 2).chain(std::iter::repeat_n(1, count * 2 + 2))
            {
                browser.step(delta, &app, &tx);
                // Pending page loads must ignore extra wheel ticks.
                if browser.loading {
                    browser.step(delta, &app, &tx);
                }
                complete(&mut browser);
                if count == 0 {
                    assert!(browser.rows.is_empty());
                    continue;
                }
                expected = (expected as i32 + delta).clamp(0, count as i32 - 1) as usize;
                assert_eq!(browser.offset + browser.selected, expected);
                assert_eq!(app.get_selected_index(), browser.selected as i32);
                assert_eq!(browser.rows[browser.selected].label(), expected.to_string());
            }
            for last in [true, false] {
                browser.jump(last, &app, &tx);
                complete(&mut browser);
                assert_eq!(
                    browser.offset + browser.selected,
                    if last { count.saturating_sub(1) } else { 0 }
                );
                browser.step(if last { 1 } else { -1 }, &app, &tx);
                assert_eq!(
                    browser.offset + browser.selected,
                    if last { count.saturating_sub(1) } else { 0 }
                );
                assert!(
                    rx.try_recv().is_err(),
                    "boundary scroll must not fetch or activate"
                );
            }
        }
        // Window actions stay on the UI thread, never enqueue MPD mutations,
        // and refresh the label without resetting selection or navigation.
        let (tx, rx) = mpsc::channel();
        let mut browser = Browser {
            stack: vec![View::Window],
            rows: fetch(&View::Window, 0).unwrap(),
            ..Default::default()
        };
        let weak = app.as_weak();
        app.on_window_pin_requested(move || {
            let app = weak.upgrade().unwrap();
            app.set_window_pinned(!app.get_window_pinned());
        });
        let resets = std::rc::Rc::new(std::cell::Cell::new(0));
        let received = resets.clone();
        app.on_window_reset_requested(move || received.set(received.get() + 1));
        browser.open(0, &app, &tx);
        assert_eq!(browser.rows[0].display_label(&app), "Always on Top [On]");
        browser.open(1, &app, &tx);
        assert!(app.get_window_pinned());
        assert_eq!(resets.get(), 1);
        browser.open(0, &app, &tx);
        assert_eq!(browser.rows[0].display_label(&app), "Always on Top [Off]");
        assert_eq!(browser.view(), View::Window);
        assert!(rx.try_recv().is_err());

        // Opening Up Next is navigation only. Duplicate files retain distinct IDs.
        for action in [0, 1, 2, 4] {
            let (tx, rx) = mpsc::channel();
            let mut browser = Browser {
                stack: vec![View::Queue],
                rows: [7, 42]
                    .into_iter()
                    .map(|id| {
                        Entry::Queue(mpd::QueueSong {
                            id,
                            position: 0,
                            song: mpd::Song {
                                file: "same.flac".into(),
                                title: "Song".into(),
                                ..Default::default()
                            },
                        })
                    })
                    .collect(),
                ..Default::default()
            };
            browser.open(1, &app, &tx);
            let Job::Fetch(request) = rx.try_recv().unwrap() else {
                panic!("opening a song must not mutate")
            };
            let rows = fetch(&request.view, request.offset).unwrap();
            assert_eq!(
                rows.iter().map(Entry::label).collect::<Vec<_>>(),
                [
                    "Play Now",
                    "Play Next",
                    "Add to Queue",
                    "Add to Playlist",
                    "Remove from Queue"
                ]
            );
            browser.complete(&app, &tx, request, Ok(rows));
            browser.open(action, &app, &tx);
            let Job::Mutate(mutation, generation) = rx.try_recv().unwrap() else {
                panic!("expected action")
            };
            assert!(match (action, &mutation) {
                (0, Mutation::PlayQueue(42))
                | (1, Mutation::NextQueue(42))
                | (4, Mutation::Remove(42)) => true,
                (2, Mutation::Source(mpd::QueueSource::Song(file), mpd::QueueAction::Append)) =>
                    file == "same.flac",
                _ => false,
            });
            browser.finish(&app, &tx, generation, mutation, Ok(()));
            assert_eq!(browser.view(), View::Queue);
            assert!(matches!(
                rx.try_recv().unwrap(),
                Job::Fetch(Request {
                    view: View::Queue,
                    select_index: Some(1),
                    ..
                })
            ));
        }

        let (tx, rx) = mpsc::channel();
        let action = Mutation::Source(
            mpd::QueueSource::Song("song.flac".into()),
            mpd::QueueAction::Append,
        );
        let make_browser = || Browser {
            stack: vec![
                View::Songs,
                View::Actions(mpd::QueueSource::Song("song.flac".into()), "Song".into()),
            ],
            history: vec![
                Page {
                    rows: vec![],
                    offset: 0,
                    selected: 0,
                    more: false,
                },
                Page {
                    rows: vec![Entry::Info("one".into()), Entry::Info("two".into())],
                    offset: 0,
                    selected: 1,
                    more: false,
                },
            ],
            mutating: true,
            generation: 7,
            mutation_rows: vec![Entry::Action("Add to Queue", mpd::QueueAction::Append)],
            ..Default::default()
        };
        let mut successful = make_browser();
        successful.finish(&app, &tx, 7, action, Ok(()));
        assert_eq!(successful.view(), View::Songs);
        assert_eq!(successful.selected, 1);
        assert!(rx.try_recv().is_err());
        let mut failed = make_browser();
        failed.finish(
            &app,
            &tx,
            7,
            Mutation::Source(
                mpd::QueueSource::Song("song.flac".into()),
                mpd::QueueAction::Append,
            ),
            Err("offline".into()),
        );
        assert!(matches!(failed.view(), View::Actions(_, _)));
        assert_eq!(failed.rows.len(), 1);
        assert!(rx.try_recv().is_err());

        let mut naming = Browser {
            stack: vec![View::Playlists],
            rows: vec![Entry::NewPlaylist],
            ..Default::default()
        };
        naming.open(0, &app, &tx);
        assert!(app.get_playlist_entry_open());
        naming.submit_playlist_name(" ", &app, &tx);
        assert!(!app.get_playlist_entry_error().is_empty());
        assert!(rx.try_recv().is_err());
        naming.submit_playlist_name("Road", &app, &tx);
        let Job::Mutate(mutation, generation) = rx.try_recv().unwrap() else {
            panic!("expected creation");
        };
        assert!(matches!(mutation, Mutation::CreatePlaylist(_, None)));
        naming.finish(&app, &tx, generation, mutation, Err("offline".into()));
        assert!(app.get_playlist_entry_open());
        assert_eq!(app.get_playlist_entry_error(), "offline");
        naming.submit_playlist_name("Road", &app, &tx);
        let Job::Mutate(mutation, generation) = rx.try_recv().unwrap() else {
            panic!("expected retry");
        };
        naming.finish(&app, &tx, generation, mutation, Ok(()));
        assert!(!app.get_playlist_entry_open());
        assert!(matches!(rx.try_recv().unwrap(), Job::FindPlaylist(_, name) if name == "Road"));

        let mut add = Browser {
            stack: vec![
                View::Songs,
                View::Actions(mpd::QueueSource::Song("song.flac".into()), "Song".into()),
                View::AddToPlaylist("song.flac".into()),
            ],
            history: vec![
                Page {
                    rows: vec![],
                    offset: 0,
                    selected: 0,
                    more: false,
                },
                Page {
                    rows: vec![Entry::Info("first".into()), Entry::Info("selected".into())],
                    offset: 0,
                    selected: 1,
                    more: false,
                },
                Page {
                    rows: vec![Entry::Navigate(
                        "Add to Playlist",
                        View::AddToPlaylist("song.flac".into()),
                    )],
                    offset: 0,
                    selected: 0,
                    more: false,
                },
            ],
            rows: vec![Entry::NewPlaylist],
            ..Default::default()
        };
        add.open(0, &app, &tx);
        add.submit_playlist_name("Together", &app, &tx);
        let Job::Mutate(mutation, generation) = rx.try_recv().unwrap() else {
            panic!("expected add");
        };
        assert!(matches!(mutation, Mutation::CreatePlaylist(_, Some(_))));
        add.finish(&app, &tx, generation, mutation, Ok(()));
        assert_eq!(add.view(), View::Songs);
        assert_eq!(add.selected, 1);
        assert!(rx.try_recv().is_err());

        let picker_song = |index: usize| {
            Entry::PickerSong(mpd::Song {
                file: format!("{index}.flac"),
                title: format!("Song {index}"),
                ..Default::default()
            })
        };
        let mut picker = Browser {
            stack: vec![
                View::Playlists,
                View::Actions(mpd::QueueSource::Playlist("List".into()), "List".into()),
                View::SelectMusic("List".into()),
            ],
            history: vec![
                Page {
                    rows: vec![Entry::Playlist("List".into())],
                    offset: 0,
                    selected: 0,
                    more: false,
                },
                Page {
                    rows: vec![Entry::Navigate(
                        "Select Music",
                        View::SelectMusic("List".into()),
                    )],
                    offset: 0,
                    selected: 0,
                    more: false,
                },
            ],
            rows: std::iter::once(Entry::CommitSelected)
                .chain((0..5).map(picker_song))
                .collect(),
            selected: 1,
            more: true,
            picker_cache: (0..5).map(|i| (i, format!("{i}.flac"))).collect(),
            ..Default::default()
        };
        picker.show(&app, "");
        picker.picker_space(&app, &tx);
        assert_eq!(picker.selected, 2);
        assert_eq!(picker.picker_files.keys().copied().collect::<Vec<_>>(), [0]);
        picker.shift_picker(1, &app, &tx);
        assert_eq!(
            picker.picker_files.keys().copied().collect::<Vec<_>>(),
            [0, 1, 2]
        );
        picker.picker_bulk(false, &app, &tx);
        assert!(matches!(rx.try_recv().unwrap(), Job::BulkSelect(_, false)));
        picker.complete_bulk(
            &app,
            picker.generation,
            false,
            Ok((0..8).map(|i| format!("{i}.flac")).collect()),
        );
        assert_eq!(picker.picker_files.len(), 8);
        picker.picker_bulk(true, &app, &tx);
        assert!(matches!(rx.try_recv().unwrap(), Job::BulkSelect(_, true)));
        picker.complete_bulk(
            &app,
            picker.generation,
            true,
            Ok((0..8).map(|i| format!("{i}.flac")).collect()),
        );
        assert!(picker.picker_files.is_empty());
        picker.selected = 5;
        picker.show(&app, "");
        picker.shift_picker(1, &app, &tx);
        let Job::Fetch(request) = rx.try_recv().unwrap() else {
            panic!("next picker page");
        };
        assert_eq!(request.offset, 6);
        picker.complete(&app, &tx, request, Ok((5..11).map(picker_song).collect()));
        assert_eq!(
            picker.picker_files.keys().copied().collect::<Vec<_>>(),
            [4, 5]
        );
        picker.picker_bulk(true, &app, &tx);
        assert!(matches!(rx.try_recv().unwrap(), Job::BulkSelect(_, true)));
        picker.complete_bulk(
            &app,
            picker.generation,
            true,
            Ok((0..8).map(|i| format!("{i}.flac")).collect()),
        );
        assert_eq!(picker.picker_files.len(), 6);
        picker.rows = std::iter::once(Entry::CommitSelected)
            .chain((0..5).map(picker_song))
            .collect();
        picker.offset = 0;
        picker.selected = 0;
        picker.show(&app, "");
        picker.open(0, &app, &tx);
        let Job::Mutate(mut mutation, generation) = rx.try_recv().unwrap() else {
            panic!("batch add");
        };
        if let Mutation::AddSelectedMusic { added, files, .. } = &mut mutation {
            assert_eq!(files.len(), 6);
            *added = 2;
        } else {
            panic!("wrong mutation");
        }
        picker.finish(
            &app,
            &tx,
            generation,
            mutation,
            Err("partial addition".into()),
        );
        assert_eq!(picker.view(), View::SelectMusic("List".into()));
        assert_eq!(picker.picker_files.len(), 4);
        picker.open(0, &app, &tx);
        let Job::Mutate(mutation, generation) = rx.try_recv().unwrap() else {
            panic!("retry");
        };
        assert!(matches!(&mutation, Mutation::AddSelectedMusic { files, .. } if files.len() == 4));
        picker.finish(&app, &tx, generation, mutation, Ok(()));
        assert_eq!(picker.view(), View::PlaylistSongs("List".into()));
        assert!(matches!(
            rx.try_recv().unwrap(),
            Job::Fetch(Request {
                view: View::PlaylistSongs(_),
                ..
            })
        ));
    }

    #[test]
    fn window_menu_is_local_and_uses_the_existing_leaf_actions() {
        let rows = fetch(&View::Window, 0).unwrap();
        assert_eq!(
            rows.iter().map(Entry::label).collect::<Vec<_>>(),
            ["Always on Top", "Reset Window Size"]
        );
        assert!(rows.iter().all(Entry::is_leaf));
        assert!(matches!(rows[0], Entry::PinWindow));
        assert!(matches!(rows[1], Entry::ResetWindow));
    }

    #[test]
    fn end_navigation_handles_empty_exact_and_partial_pages() {
        for count in [0usize, 1, 5, 6, 7, 12, 13, 10000] {
            let mut calls = 0;
            let (offset, rows) = last_page(|offset| {
                calls += 1;
                Ok((offset..count).take(PAGE + 1).collect::<Vec<_>>())
            })
            .unwrap();
            assert_eq!(offset, count.saturating_sub(1) / PAGE * PAGE);
            assert_eq!(rows.last().copied(), count.checked_sub(1));
            assert!(rows.len() <= PAGE);
            assert!(calls < 30);
        }
        assert!(last_page::<Entry>(|_| Err("offline".into())).is_err());
    }

    #[test]
    fn settings_and_saved_playlist_actions_are_separate_from_queue() {
        let song = fetch(
            &View::Actions(mpd::QueueSource::Song("song.flac".into()), "Song".into()),
            0,
        )
        .unwrap();
        assert_eq!(
            song.iter().map(Entry::label).collect::<Vec<_>>(),
            ["Play Now", "Play Next", "Add to Queue", "Add to Playlist"]
        );
        assert!(
            matches!(&song[3], Entry::Navigate(_, View::AddToPlaylist(file)) if file == "song.flac")
        );
        assert!(!song[3].is_leaf());
        assert!(Entry::PlaylistTarget("Saved".into(), "song.flac".into()).is_leaf());
        let settings = fetch(&View::Settings, 0).unwrap();
        assert_eq!(
            settings.iter().map(Entry::label).collect::<Vec<_>>(),
            [
                "Playback",
                #[cfg(target_os = "macos")]
                "Window",
                "Controls (future)",
                "Appearance (future)",
                "About"
            ]
        );
        assert_eq!(fetch(&View::Playback, 0).unwrap().len(), 4);
        assert_eq!(fetch(&View::Crossfade, 0).unwrap().len(), 6);
        assert_eq!(fetch(&View::Repeat, 0).unwrap().len(), 2);
        assert_eq!(fetch(&View::Random, 0).unwrap().len(), 2);
        assert!(!Mutation::AddToPlaylist("Saved".into(), "song.flac".into()).starts_playback());
    }

    #[test]
    fn feedback_describes_only_completed_supported_mutations() {
        use mpd::{PlaybackSetting as S, QueueAction as A, QueueSource as Q};
        let saved = Mutation::AddToPlaylist("Saved".into(), "song.flac".into());
        assert_eq!(saved.feedback(true), Some("Added to playlist"));
        assert_eq!(saved.feedback(false), Some("Unable to add to playlist"));
        let song = Q::Song("song.flac".into());
        assert_eq!(
            Mutation::Source(song.clone(), A::Append).feedback(true),
            Some("Added to queue")
        );
        assert_eq!(
            Mutation::Source(song, A::PlayNext).feedback(true),
            Some("Playing next")
        );
        assert_eq!(
            Mutation::Source(Q::Artist("Artist".into()), A::PlayNow).feedback(true),
            Some("Queue replaced")
        );
        assert_eq!(Mutation::Clear.feedback(true), Some("Queue cleared"));
        assert_eq!(
            Mutation::Setting(S::Repeat(true)).feedback(true),
            Some("Repeat On")
        );
        assert_eq!(
            Mutation::Setting(S::Random(false)).feedback(true),
            Some("Shuffle Off")
        );
        assert_eq!(Mutation::Clear.feedback(false), None);
    }

    #[test]
    fn music_is_the_single_library_entry_point() {
        let music = fetch(&View::Music, 0).unwrap();
        assert_eq!(
            music.iter().map(Entry::label).collect::<Vec<_>>(),
            ["Playlists", "Artists", "Albums", "Songs", "Browse Files"]
        );
        let home = fetch(&View::Home, 0).unwrap();
        assert!(home.len() <= PAGE);
        assert!(matches!(home[3], Entry::Navigate(_, View::Queue)));
        assert!(matches!(home[4], Entry::Navigate(_, View::NowPlaying)));
    }

    #[test]
    fn every_collection_has_browse_play_shuffle_next_and_append() {
        use mpd::{QueueAction as A, QueueSource as S};
        for source in [
            S::Album("Album".into(), None),
            S::Playlist("List".into()),
            S::Artist("Artist".into()),
            S::Folder("root/child".into()),
        ] {
            let rows = fetch(&View::Actions(source.clone(), "Title".into()), 0).unwrap();
            let actions_start = if matches!(source, S::Playlist(_)) {
                2
            } else {
                1
            };
            assert_eq!(rows.len(), actions_start + 4);
            assert!(!rows[0].is_leaf());
            if actions_start == 2 {
                assert!(
                    matches!(&rows[1], Entry::Navigate("Select Music", View::SelectMusic(name)) if name == "List")
                );
            }
            for (row, action) in
                rows[actions_start..]
                    .iter()
                    .zip([A::PlayNow, A::Shuffle, A::PlayNext, A::Append])
            {
                assert!(matches!(row, Entry::Action(_, value) if *value == action));
                assert!(row.is_leaf());
            }
            assert!(rows[actions_start + 1].is_shuffle());
            assert!(Mutation::Source(source.clone(), A::Shuffle).starts_playback());
            assert!(!Mutation::Source(source, A::Append).starts_playback());
        }
        let rows = fetch(
            &View::Actions(S::Folder("root/child".into()), "child".into()),
            0,
        )
        .unwrap();
        assert!(
            matches!(&rows[0], Entry::Navigate(_, View::Directory(path)) if path == "root/child")
        );
    }

    #[test]
    fn queue_pages_account_for_the_action_header_without_skipping_songs() {
        assert_eq!(queue_window(0), (0, 6)); // header + five visible songs + lookahead
        assert_eq!(queue_window(6), (5, 7));
        assert_eq!(queue_window(12), (11, 7));
    }

    #[test]
    fn queue_shuffle_is_non_transport_and_randomise_replaces_and_plays() {
        let rows = fetch(&View::QueueActions, 0).unwrap();
        assert_eq!(
            rows.iter().map(Entry::label).collect::<Vec<_>>(),
            ["Shuffle Queue", "Clear Queue…"]
        );
        assert!(matches!(rows[0], Entry::ShuffleQueue));
        assert!(rows[0].is_leaf() && rows[0].is_shuffle());
        assert!(!Mutation::ShuffleQueue.starts_playback());
        assert!(Mutation::RandomisePlay.starts_playback());
        assert_eq!(
            Mutation::ShuffleQueue.feedback(true),
            Some("Queue shuffled")
        );
        assert_eq!(Mutation::ShuffleQueue.feedback(false), None);
        assert_eq!(
            Mutation::RandomisePlay.feedback(true),
            Some("Queue replaced")
        );
    }

    #[test]
    fn home_order_and_pagination() {
        let rows = Entry::home_menu();
        assert_eq!(
            rows.iter().map(Entry::label).collect::<Vec<_>>(),
            [
                "Music",
                "Randomise Play",
                "Shuffle Queue",
                "Queue / Up Next",
                "Now Playing",
                "Settings"
            ]
        );
        assert_eq!(page(rows.clone(), 0).len(), PAGE);
        assert_eq!(page(rows, 0)[3].label(), "Queue / Up Next");
    }
    #[test]
    fn row_semantics_and_action_pages() {
        assert!(!Entry::Folder("a/b".into()).is_leaf());
        assert_eq!(Entry::Folder("a/b".into()).label(), "b");
        assert!(Entry::Song(mpd::Song::default()).is_leaf());
        assert!(Entry::ShuffleQueue.is_shuffle());
        assert!(Entry::RandomisePlay.is_shuffle());
        assert!(Entry::Action("Play Next", mpd::QueueAction::PlayNext).is_leaf());
        assert!(!Entry::Action("Play Next", mpd::QueueAction::PlayNext).is_song());
        let view = View::Actions(
            mpd::QueueSource::Album("Album".into(), Some("Artist".into())),
            "Album".into(),
        );
        let rows = fetch(&view, 0).unwrap();
        assert_eq!(
            rows.iter().map(Entry::label).collect::<Vec<_>>(),
            [
                "Browse Songs",
                "Play Album",
                "Shuffle Album",
                "Play Next",
                "Add Album to Queue"
            ]
        );
        assert!(
            matches!(&rows[0], Entry::Navigate(_, View::AlbumSongs(_, Some(a))) if a == "Artist")
        );
        assert!(matches!(
            fetch(&View::ConfirmClear, 0).unwrap()[0],
            Entry::Cancel
        ));
    }
}
