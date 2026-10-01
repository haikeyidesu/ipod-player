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
    RemoveQueue,
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
    Directory(String),
    Actions(mpd::QueueSource, String),
    AddToPlaylist(String),
    Settings,
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
            Self::Settings => "Settings",
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
            Self::RemoveQueue => "Remove Item",
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
    ShuffleSongs,
    Queue(mpd::QueueSong),
    Remove(mpd::QueueSong),
    Artist(String),
    Album(String),
    Song(mpd::Song),
    Playlist(String),
    Folder(String),
    Action(&'static str, mpd::QueueAction),
    Clear,
    Cancel,
    PlaylistTarget(String, String),
    Setting(String, mpd::PlaybackSetting),
    Info(String),
}
impl Entry {
    fn home_menu() -> Vec<Self> {
        vec![
            Self::Navigate("Music", View::Music),
            Self::ShuffleSongs,
            Self::Navigate("Queue / Up Next", View::Queue),
            Self::Navigate("Now Playing", View::NowPlaying),
            Self::Navigate("Settings", View::Settings),
        ]
    }
    fn label(&self) -> &str {
        match self {
            Self::Navigate(label, _) | Self::Action(label, _) => label,
            Self::ShuffleSongs => "Shuffle Songs",
            Self::Clear => "Clear Queue",
            Self::Cancel => "Cancel",
            Self::Queue(s) | Self::Remove(s) => &s.song.title,
            Self::Artist(n) | Self::Album(n) | Self::Playlist(n) => n,
            Self::Folder(path) => friendly(path),
            Self::Song(s) => &s.title,
            Self::PlaylistTarget(name, _) | Self::Setting(name, _) | Self::Info(name) => name,
        }
    }
    fn display_label(&self, app: &AppWindow) -> String {
        use mpd::PlaybackSetting as S;
        match self {
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
                | Self::ShuffleSongs
                | Self::Action(_, _)
                | Self::Clear
                | Self::Cancel
                | Self::PlaylistTarget(_, _)
                | Self::Setting(_, _)
                | Self::Info(_)
        )
    }
    fn is_shuffle(&self) -> bool {
        matches!(
            self,
            Self::ShuffleSongs | Self::Action(_, mpd::QueueAction::Shuffle)
        )
    }
    fn is_song(&self) -> bool {
        matches!(self, Self::Song(_) | Self::Queue(_) | Self::Remove(_))
    }
    fn queue_position(&self) -> i32 {
        match self {
            Self::Queue(s) | Self::Remove(s) => i32::try_from(s.position).unwrap_or(-1),
            _ => -1,
        }
    }
}

struct Request {
    view: View,
    offset: usize,
    generation: u64,
    select_last: bool,
}
enum Mutation {
    AddToPlaylist(String, String),
    Setting(mpd::PlaybackSetting),
    Source(mpd::QueueSource, mpd::QueueAction),
    PlayQueue(u64),
    Remove(u64),
    Clear,
    Shuffle,
}
impl Mutation {
    fn run(&self) -> Result<(), String> {
        match self {
            Self::Source(source, action) => mpd::queue_source(source, *action),
            Self::PlayQueue(id) => mpd::play_queue_id(*id),
            Self::Remove(id) => mpd::remove_queue_id(*id),
            Self::Clear => mpd::clear_queue(),
            Self::AddToPlaylist(name, file) => mpd::add_to_playlist(name, file),
            Self::Setting(setting) => mpd::set_playback(*setting),
            Self::Shuffle => mpd::shuffle_all_songs(),
        }
    }
    fn starts_playback(&self) -> bool {
        matches!(
            self,
            Self::Source(_, mpd::QueueAction::PlayNow | mpd::QueueAction::Shuffle) | Self::Shuffle
        )
    }
}
enum Job {
    JumpLast(Request),
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
}
impl Browser {
    fn view(&self) -> View {
        self.stack.last().cloned().unwrap_or(View::Home)
    }
    fn show(&self, app: &AppWindow, message: &str) {
        let title = if self.view() == View::Volume {
            if app.get_player_volume() < 0 {
                "Volume unavailable".into()
            } else {
                format!("Volume: {}%", app.get_player_volume())
            }
        } else {
            self.view().title().to_string()
        };
        app.set_page_title(title.into());
        app.set_browse_active(self.view() != View::NowPlaying);
        app.set_browse_message(message.into());
        app.set_browser_items(ModelRc::from(std::rc::Rc::new(VecModel::from(
            self.rows
                .iter()
                .map(|r| SharedString::from(r.display_label(app)))
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
        };
        self.show(app, "Loading…");
        if tx.send(Job::Fetch(request)).is_err() {
            self.loading = false;
            self.show(app, "Browser worker stopped");
        }
    }
    fn push(&mut self, view: View, app: &AppWindow, tx: &mpsc::Sender<Job>) {
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
        } else {
            let offset = if self.view() == View::Volume {
                app.get_player_volume().max(0) as usize / PAGE * PAGE
            } else {
                0
            };
            self.fetch(app, tx, offset, false);
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
    fn open(&mut self, index: i32, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if self.loading || index < 0 {
            return;
        }
        let Some(entry) = self.rows.get(index as usize).cloned() else {
            return;
        };
        self.selected = index as usize;
        match entry {
            Entry::Navigate(_, view) => self.push(view, app, tx),
            Entry::ShuffleSongs => self.mutate(Mutation::Shuffle, app, tx),
            Entry::Queue(song) => self.mutate(Mutation::PlayQueue(song.id), app, tx),
            Entry::Remove(song) => self.mutate(Mutation::Remove(song.id), app, tx),
            Entry::Clear => self.mutate(Mutation::Clear, app, tx),
            Entry::Cancel => self.back(app, tx),
            Entry::PlaylistTarget(name, file) => {
                self.mutate(Mutation::AddToPlaylist(name, file), app, tx)
            }
            Entry::Setting(_, setting) => self.mutate(Mutation::Setting(setting), app, tx),
            Entry::Info(_) => {}
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
                if let View::Actions(source, _) = self.view() {
                    self.mutate(Mutation::Source(source, action), app, tx);
                }
            }
        }
    }
    fn back(&mut self, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        // Avoid executing a second action while the first is still in flight.
        if self.mutating {
            return;
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
            View::Queue | View::RemoveQueue | View::PlaylistSongs(_) | View::AddToPlaylist(_)
        ) {
            self.fetch(app, tx, self.offset, false);
        } else {
            self.show(app, "");
        }
    }
    fn jump(&mut self, last: bool, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if self.loading || self.view() == View::NowPlaying {
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
        };
        if tx.send(Job::JumpLast(request)).is_err() {
            self.loading = false;
            self.show(app, "Browser worker stopped");
        }
    }

    fn step(&mut self, delta: i32, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if self.loading || self.rows.is_empty() {
            return;
        }
        // Mouse selection and wheel selection must share the same index.
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
                self.selected = if request.select_last {
                    self.rows.len().saturating_sub(1)
                } else if request.view == View::Volume {
                    self.rows.iter().position(|entry| matches!(entry, Entry::Setting(_, mpd::PlaybackSetting::Volume(v)) if i32::from(*v) == app.get_player_volume())).unwrap_or(0)
                } else {
                    0
                };
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
        match result {
            Ok(()) => {
                if mutation.starts_playback() {
                    // Restore the action/home menu before saving it in navigation history.
                    self.rows = std::mem::take(&mut self.mutation_rows);
                    self.push(View::NowPlaying, app, tx);
                } else if matches!(mutation, Mutation::AddToPlaylist(_, _)) {
                    self.back(app, tx);
                } else if matches!(mutation, Mutation::Clear) {
                    // Leave the confirmation and action pages, returning to the live queue.
                    while self.view() != View::Queue && !self.stack.is_empty() {
                        self.stack.pop();
                        self.history.pop();
                    }
                    self.fetch(app, tx, 0, false);
                } else {
                    self.fetch(app, tx, self.offset, false);
                }
            }
            Err(err) => {
                eprintln!("MPD queue action: {err}");
                self.show(app, &format!("{err}\nMENU to go back"));
            }
        }
    }
    fn refresh_queue(&mut self, app: &AppWindow, tx: &mpsc::Sender<Job>) {
        if !self.mutating && matches!(self.view(), View::Queue | View::RemoveQueue) {
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
        View::RemoveQueue => Some(mpd::queue_length()?),
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
        View::Volume => {
            if mpd::read_status()?.volume.is_none() {
                vec![Entry::Info("MPD mixer unavailable".into())]
            } else {
                page(
                    (0..=100)
                        .map(|v| Entry::Setting(format!("{v}%"), mpd::PlaybackSetting::Volume(v)))
                        .collect(),
                    offset,
                )
            }
        }
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
        View::AddToPlaylist(file) => page(
            mpd::list_playlists()?
                .into_iter()
                .map(|p| Entry::PlaylistTarget(p.name, file.clone()))
                .collect(),
            offset,
        ),
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
                Entry::Navigate("Remove from Queue", View::RemoveQueue),
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
        View::Playlists => page(
            mpd::list_playlists()?
                .into_iter()
                .map(|p| Entry::Playlist(p.name))
                .collect(),
            offset,
        ),
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
        View::RemoveQueue => mpd::queue_songs(offset, limit)?
            .into_iter()
            .map(Entry::Remove)
            .collect(),
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
                Job::Mutate(mutation, generation) => {
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
    let settings_requests = tx.clone();
    app.on_refresh_settings(move || {
        if let Some(app) = settings_weak.upgrade() {
            let mut controller = settings_controller.lock().unwrap();
            if !controller.loading
                && matches!(
                    controller.view(),
                    View::Playback | View::Volume | View::Crossfade | View::Repeat | View::Random
                )
            {
                if controller.view() == View::Volume && !controller.rows.is_empty() {
                    let unavailable = matches!(controller.rows[0], Entry::Info(_));
                    if unavailable != (app.get_player_volume() < 0) {
                        controller.fetch(&app, &settings_requests, 0, false);
                        return;
                    }
                }
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
    fn music_is_the_single_library_entry_point() {
        let music = fetch(&View::Music, 0).unwrap();
        assert_eq!(
            music.iter().map(Entry::label).collect::<Vec<_>>(),
            ["Playlists", "Artists", "Albums", "Songs", "Browse Files"]
        );
        let home = fetch(&View::Home, 0).unwrap();
        assert!(home.len() <= PAGE);
        assert!(matches!(home[2], Entry::Navigate(_, View::Queue)));
        assert!(matches!(home[3], Entry::Navigate(_, View::NowPlaying)));
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
            assert_eq!(rows.len(), 5);
            assert!(!rows[0].is_leaf());
            for (row, action) in
                rows[1..]
                    .iter()
                    .zip([A::PlayNow, A::Shuffle, A::PlayNext, A::Append])
            {
                assert!(matches!(row, Entry::Action(_, value) if *value == action));
                assert!(row.is_leaf());
            }
            assert!(rows[2].is_shuffle());
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
    fn home_order_and_pagination() {
        let rows = Entry::home_menu();
        assert_eq!(
            rows.iter().map(Entry::label).collect::<Vec<_>>(),
            [
                "Music",
                "Shuffle Songs",
                "Queue / Up Next",
                "Now Playing",
                "Settings"
            ]
        );
        assert_eq!(page(rows.clone(), 0).len(), 5);
        assert_eq!(page(rows, 0)[2].label(), "Queue / Up Next");
    }
    #[test]
    fn row_semantics_and_action_pages() {
        assert!(!Entry::Folder("a/b".into()).is_leaf());
        assert_eq!(Entry::Folder("a/b".into()).label(), "b");
        assert!(Entry::Song(mpd::Song::default()).is_leaf());
        assert!(Entry::ShuffleSongs.is_shuffle());
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
