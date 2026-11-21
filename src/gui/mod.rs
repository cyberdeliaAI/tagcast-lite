//! The desktop window: sign-in, library browser, queue and now-playing bar.

mod artwork;
mod catalog;
mod listening;
mod media;
mod queue;
mod theme;
mod views;

use std::{
    collections::HashSet,
    error::Error,
    sync::{
        Arc,
        mpsc::{Receiver, Sender, channel},
    },
    thread,
    time::{Duration, Instant},
};

use eframe::egui::{self, Key, Margin, RichText};
use serde::{Deserialize, Serialize};
use tagcast_player::{
    APP_NAME, CancellationToken, Failure, Result,
    account::{Account, Http, LibraryUpdate, Session, Track, logout},
    audio::{NextTrack, Output, Phase, Player},
    transport::{MediaRequest, Quality},
};

use self::{
    artwork::Artwork,
    catalog::{
        AlbumSort, ArtistRole, ArtistSort, BrowseGroup, BrowseKind, BrowseSort, Catalog,
        SearchResults, SongSort,
    },
    queue::{Queue, Removed},
};

pub struct Options {
    pub client_id: Option<String>,
}

pub fn run(options: Options) -> std::result::Result<(), Box<dyn Error>> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_app_id("tagcast-player")
        .with_inner_size([1200.0, 780.0])
        .with_min_inner_size([820.0, 540.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon-256.png"))
    {
        viewport = viewport.with_icon(icon);
    }
    #[cfg(debug_assertions)]
    let fixture_preferences = std::env::var_os("TAGCAST_PLAYER_FIXTURE")
        .map(|_| tempfile::NamedTempFile::new())
        .transpose()?;
    let native = eframe::NativeOptions {
        viewport,
        #[cfg(debug_assertions)]
        persistence_path: fixture_preferences
            .as_ref()
            .map(|file| file.path().to_owned()),
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        native,
        Box::new(move |cc| Ok(Box::new(App::new(cc, options)))),
    )?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Repeat {
    Off,
    All,
    One,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    /// Slider position, 0 to 1. The output gain is its square.
    volume: f32,
    muted: bool,
    quality: String,
    repeat: Repeat,
    shuffle: bool,
    album_sort: AlbumSort,
    album_sort_reversed: bool,
    artist_sort: ArtistSort,
    track_artist_sort: ArtistSort,
    gapless: bool,
    queue_panel: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: 0.9,
            muted: false,
            quality: "128".into(),
            repeat: Repeat::Off,
            shuffle: false,
            album_sort: AlbumSort::Artist,
            album_sort_reversed: false,
            artist_sort: ArtistSort::default(),
            track_artist_sort: ArtistSort::default(),
            gapless: false,
            queue_panel: false,
        }
    }
}

pub const QUALITIES: [Quality; 6] = [
    Quality::K128,
    Quality::K96,
    Quality::K192,
    Quality::K256,
    Quality::K320,
    Quality::Original,
];

pub fn quality_label(quality: Quality) -> &'static str {
    match quality {
        Quality::Original => "Original",
        Quality::K96 => "96 kbps",
        Quality::K128 => "128 kbps",
        Quality::K192 => "192 kbps",
        Quality::K256 => "256 kbps",
        Quality::K320 => "320 kbps",
    }
}

impl Settings {
    fn quality(&self) -> Quality {
        QUALITIES
            .into_iter()
            .find(|q| q.value() == self.quality)
            .unwrap_or(Quality::K128)
    }

    fn gain(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            self.volume.clamp(0.0, 1.0).powi(2)
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Page {
    Songs,
    Albums,
    Artists(ArtistRole),
    Album(u64),
    Artist(ArtistRole, String),
    Playlist(usize),
    Queue,
    Search,
    Browse,
    BrowseAlbums(BrowseGroup),
    Collection(Collection),
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Collection {
    Favourites,
    Recent,
}

/// What a view asks for. Applied after drawing, so views only borrow what they show.
pub enum Action {
    Go(Page),
    Back,
    /// `shuffle`: `Some(true)` for a Shuffle button, `Some(false)` for Play, `None` keeps the mode.
    Play {
        tracks: Vec<usize>,
        start: Option<usize>,
        shuffle: Option<bool>,
    },
    Enqueue {
        tracks: Vec<usize>,
        next: bool,
    },
    QueueJump(usize),
    QueueRemove(usize),
    QueueClear,
    TogglePlay,
    Next,
    Previous,
    Seek(f64),
    SetShuffle(bool),
    CycleRepeat,
    SetQuality(Quality),
    Reload,
    SignOut,
    ToggleFavourite(u64),
    ToggleGapless,
    RecoverOutput,
    ToggleQueuePanel,
}

enum Message {
    Code(String, String),
    Connected(Result<Arc<Account>>),
    Cache(Box<Catalog>),
    Library(Result<CatalogUpdate>),
}

enum CatalogUpdate {
    Current { premium: bool },
    Replaced(Box<Catalog>),
}

#[derive(Default)]
struct Browse {
    kind: BrowseKind,
    sort: BrowseSort,
    query: String,
    rows: Option<Vec<usize>>,
    album_rows: Option<BrowseAlbumRows>,
}

struct BrowseAlbumRows {
    group: BrowseGroup,
    slots: HashSet<usize>,
    order: Option<(Arc<Vec<usize>>, Vec<usize>)>,
}

#[derive(Default)]
struct ArtistBrowser {
    query: String,
    rows: Option<Vec<usize>>,
}

#[derive(Clone)]
enum Source {
    Account(Arc<Account>),
    #[cfg(debug_assertions)]
    Fixture {
        http: Http,
        audio: Option<tagcast_player::Url>,
    },
}

enum Stage {
    SignIn {
        code: Option<(String, String)>,
        error: Option<Failure>,
        stop: CancellationToken,
        busy: bool,
    },
    Ready,
}

pub struct App {
    client_id: Option<String>,
    ctx: egui::Context,
    send: Sender<(u64, Message)>,
    receive: Receiver<(u64, Message)>,
    generation: u64,
    session: Session,
    stage: Stage,
    source: Option<Source>,
    catalog: Option<Catalog>,
    loading: Option<CancellationToken>,
    library_error: Option<Failure>,
    player: Player,
    _stream: Option<cpal::Stream>,
    audio_error: Option<String>,
    queue: Queue,
    listening: listening::Listening,
    listening_owner: String,
    listening_writer: listening::Writer,
    listening_dirty: Option<Instant>,
    media: Option<media::Media>,
    media_attempted: bool,
    browse: Browse,
    artist_browsers: [ArtistBrowser; 2],
    collection_rows: Option<(Collection, Arc<Vec<usize>>)>,
    ending_handled: bool,
    ordered_album: bool,
    settings: Settings,
    page: Page,
    history: Vec<Page>,
    query: String,
    search: Option<(String, Arc<SearchResults>)>,
    search_focus: bool,
    search_all_albums: bool,
    song_sort: (SongSort, bool),
    selected: Option<u64>,
    art: Artwork,
    logo: Option<egui::TextureHandle>,
    /// A requested position, shown until playback reaches it.
    seeking: Option<(f64, Instant)>,
    title: String,
    toast: Option<(String, Instant)>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, options: Options) -> Self {
        theme::install(&cc.egui_ctx);
        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, "settings"))
            .unwrap_or_default();
        let (output, stream, audio_error) = match Output::device() {
            Ok((output, stream)) => (output, Some(stream), None),
            Err(error) => (
                Output::new(48_000),
                None,
                Some(format!("No audio output is available ({error})")),
            ),
        };
        output.set_volume(settings.gain());
        let player = Player::new(output).expect("could not start the audio decoder thread");
        let logo = artwork::decode(include_bytes!("../../assets/icon-256.png"), 96).map(|image| {
            cc.egui_ctx
                .load_texture("logo", image, egui::TextureOptions::LINEAR)
        });
        let (send, receive) = channel();
        let mut app = Self {
            client_id: options.client_id,
            ctx: cc.egui_ctx.clone(),
            send,
            receive,
            generation: 0,
            session: Session::default(),
            stage: Stage::Ready,
            source: None,
            catalog: None,
            loading: None,
            library_error: None,
            player,
            _stream: stream,
            audio_error,
            queue: Queue::default(),
            listening: listening::Listening::default(),
            listening_owner: String::new(),
            listening_writer: listening::Writer::new(),
            listening_dirty: None,
            media: None,
            media_attempted: false,
            browse: Browse::default(),
            artist_browsers: Default::default(),
            collection_rows: None,
            ending_handled: false,
            ordered_album: false,
            settings,
            page: Page::Albums,
            history: Vec::new(),
            query: String::new(),
            search: None,
            search_focus: false,
            search_all_albums: false,
            song_sort: (SongSort::Artist, false),
            selected: None,
            art: Artwork::new(cc.egui_ctx.clone()),
            logo,
            seeking: None,
            title: String::new(),
            toast: None,
        };
        #[cfg(debug_assertions)]
        if app.open_fixture() {
            return app;
        }
        app.connect();
        app
    }

    /// Development aid: `TAGCAST_PLAYER_FIXTURE=library.json` shows a saved
    /// iBroadcast library response without signing in, and
    /// `TAGCAST_PLAYER_FIXTURE_AUDIO=http://127.0.0.1:…/file.wav` plays that file for every track.
    #[cfg(debug_assertions)]
    fn open_fixture(&mut self) -> bool {
        let Some(path) = std::env::var_os("TAGCAST_PLAYER_FIXTURE") else {
            return false;
        };
        if path == "signin" {
            self.stage = Stage::SignIn {
                code: Some((
                    "https://oauth.ibroadcast.com/device".into(),
                    "TCP-4821".into(),
                )),
                error: None,
                stop: CancellationToken::new(),
                busy: true,
            };
            return true;
        }
        let library = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .and_then(|value| tagcast_player::account::Library::parse(&value).ok());
        let Some(mut library) = library else {
            self.library_error = Some(Failure::Invalid("The fixture library is unreadable"));
            self.stage = Stage::Ready;
            return true;
        };
        library.owner = "1".into();
        library.premium = true;
        if let Ok(server) = std::env::var("TAGCAST_PLAYER_FIXTURE_ART") {
            library.fixture_artwork_server(&server);
        }
        let audio = std::env::var("TAGCAST_PLAYER_FIXTURE_AUDIO")
            .ok()
            .and_then(|url| url.parse().ok());
        self.source = Http::new().ok().map(|http| Source::Fixture { http, audio });
        if self._stream.is_none() {
            // Without an audio device, drain the output in real time so playback still advances.
            let output = self.player.output.clone();
            self.audio_error = None;
            thread::spawn(move || {
                let mut block = vec![[0.0f32; 2]; (output.rate / 100) as usize];
                loop {
                    output.render_fixture(&mut block);
                    thread::sleep(Duration::from_millis(10));
                }
            });
        }
        self.catalog = Some(Catalog::new(library));
        self.stage = Stage::Ready;
        true
    }

    fn connect(&mut self) {
        self.session.close();
        self.session = Session::default();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let session = self.session.clone();
        let stop = CancellationToken::new();
        self.stage = Stage::SignIn {
            code: None,
            error: None,
            stop: stop.clone(),
            busy: true,
        };
        let (send, ctx, client_id) = (self.send.clone(), self.ctx.clone(), self.client_id.clone());
        thread::spawn(move || {
            let result = Http::new().and_then(|http| {
                Account::connect(http, client_id, session, &stop, |uri, code| {
                    let _ = send.send((generation, Message::Code(uri.into(), code.into())));
                    ctx.request_repaint();
                })
            });
            let _ = send.send((generation, Message::Connected(result)));
            ctx.request_repaint();
        });
    }

    fn load_library(&mut self, force: bool) {
        if self.loading.is_some() {
            return;
        }
        let Some(Source::Account(account)) = &self.source else {
            return;
        };
        let stop = CancellationToken::new();
        self.loading = Some(stop.clone());
        let (account, send, ctx) = (account.clone(), self.send.clone(), self.ctx.clone());
        let generation = self.generation;
        let mut stamp = self
            .catalog
            .as_ref()
            .filter(|_| !force)
            .map(|catalog| catalog.library.stamp());
        thread::spawn(move || {
            if stamp.is_none()
                && !force
                && let Some(cache) = account.cached_library()
            {
                stamp = Some(cache.stamp());
                let _ = send.send((generation, Message::Cache(Box::new(Catalog::new(cache)))));
                ctx.request_repaint();
            }
            let result = account.library(&stop, stamp).map(|update| match update {
                LibraryUpdate::Current { premium } => CatalogUpdate::Current { premium },
                LibraryUpdate::Replaced(library) => {
                    CatalogUpdate::Replaced(Box::new(Catalog::new(*library)))
                }
            });
            let _ = send.send((generation, Message::Library(result)));
            ctx.request_repaint();
        });
    }

    fn sign_out(&mut self) {
        self.flush_listening();
        self.listening_owner.clear();
        self.listening = listening::Listening::default();
        self.session.close();
        self.browse = Browse::default();
        self.artist_browsers = Default::default();
        if let Some(stop) = self.loading.take() {
            stop.cancel();
        }
        self.player.stop();
        self.queue.clear();
        self.ordered_album = false;
        self.catalog = None;
        self.source = None;
        self.library_error = None;
        self.search = None;
        self.query.clear();
        self.history.clear();
        self.page = Page::Albums;
        self.art.clear();
        if let Err(error) = logout(self.client_id.clone()) {
            self.notify(error.to_string());
        }
        self.connect();
    }

    fn handle(&mut self, generation: u64, message: Message) {
        if generation != self.generation {
            return;
        }
        match message {
            Message::Code(uri, user_code) => {
                if let Stage::SignIn { code, .. } = &mut self.stage {
                    *code = Some((uri, user_code));
                }
            }
            Message::Connected(Ok(account)) => {
                self.source = Some(Source::Account(account));
                self.stage = Stage::Ready;
                self.load_library(false);
            }
            Message::Connected(Err(failure)) => {
                if let Stage::SignIn { error, busy, .. } = &mut self.stage {
                    *busy = false;
                    *error = (failure != Failure::Cancelled).then_some(failure);
                }
            }
            Message::Cache(catalog) => self.library_ready(*catalog),
            Message::Library(result) => {
                self.loading = None;
                match result {
                    Ok(CatalogUpdate::Replaced(catalog)) => self.library_ready(*catalog),
                    Ok(CatalogUpdate::Current { premium }) => {
                        if let Some(catalog) = &mut self.catalog {
                            catalog.library.premium = premium;
                        }
                        self.library_error = None;
                    }
                    Err(Failure::Cancelled) => {}
                    Err(failure) => {
                        if self.catalog.is_some() {
                            self.notify(failure.to_string());
                        }
                        self.library_error = Some(failure);
                    }
                }
            }
        }
    }

    fn library_ready(&mut self, catalog: Catalog) {
        if self
            .catalog
            .as_ref()
            .is_some_and(|old| old.library.owner != catalog.library.owner)
        {
            self.flush_listening();
            self.player.stop();
            self.queue.clear();
            self.ordered_album = false;
            self.browse = Browse::default();
            self.artist_browsers = Default::default();
        }
        if self.listening_owner != catalog.library.owner {
            self.flush_listening();
            self.listening_owner.clone_from(&catalog.library.owner);
            self.listening = listening::Listening::load(&self.listening_owner);
            if let Some(saved) = self.listening.queue.take() {
                self.queue.restore(saved, |id| {
                    catalog
                        .track_index(id)
                        .map(|index| catalog.track(index).clone())
                });
                self.settings.shuffle = self.queue.shuffled();
            }
        }
        let valid = match &self.page {
            Page::Album(id) => catalog.album_index.contains_key(id),
            Page::Artist(role, key) => catalog.artist_by_key(key, *role).is_some(),
            Page::Playlist(index) => self
                .catalog
                .as_ref()
                .and_then(|old| old.library.playlists.get(*index))
                .zip(catalog.library.playlists.get(*index))
                .is_some_and(|(old, new)| old.id == new.id),
            _ => true,
        };
        if !valid {
            self.page = Page::Albums;
        }
        self.history.clear();
        self.search = None;
        self.collection_rows = None;
        self.browse.rows = None;
        self.browse.album_rows = None;
        for browser in &mut self.artist_browsers {
            browser.rows = None;
        }
        self.library_error = None;
        if self.catalog.is_some() {
            self.notify(format!(
                "Library updated: {}",
                catalog::count(catalog.library.tracks.len(), "song", "songs")
            ));
        }
        self.catalog = Some(catalog);
    }

    /// The playback position to show, including a seek that is still on its way.
    fn position(&self) -> f64 {
        self.seeking.map_or_else(
            || self.player.output.snapshot().position,
            |(target, _)| target,
        )
    }

    pub fn notify(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    // ---- playback ---------------------------------------------------------

    fn request(&self, track: &Arc<Track>) -> Option<MediaRequest> {
        match self.source.as_ref()? {
            Source::Account(account) => {
                let library = &self.catalog.as_ref()?.library;
                Some(MediaRequest::native(
                    account.clone(),
                    track.clone(),
                    &library.owner,
                    &library.expires,
                    self.settings.quality(),
                    library.premium,
                ))
            }
            #[cfg(debug_assertions)]
            Source::Fixture { http, audio } => MediaRequest::fixture(
                http.clone(),
                audio.clone()?,
                None,
                None,
                "wav",
                self.settings.quality(),
            )
            .ok(),
        }
    }

    fn follow_gapless(&mut self) {
        let snapshot = self.player.output.snapshot();
        if snapshot.gapless_transition
            && self
                .queue
                .current_entry()
                .is_some_and(|entry| entry.uid != snapshot.track_key)
            && self
                .queue
                .next_entry()
                .is_some_and(|entry| entry.uid == snapshot.track_key)
        {
            self.queue.advance(false);
            self.seeking = None;
            self.ending_handled = false;
            self.mark_listening();
        }
    }

    fn update_gapless(&self) {
        let next = (|| {
            if !self.settings.gapless
                || !self.ordered_album
                || self.settings.shuffle
                || self.settings.repeat == Repeat::One
                || self.settings.quality() != Quality::Original
            {
                return None;
            }
            let (current, next) = (self.queue.current_entry()?, self.queue.next_entry()?);
            let catalog = self.catalog.as_ref()?;
            let same_album =
                catalog.album_slot(&current.track)? == catalog.album_slot(&next.track)?;
            let flac = |track: &Track| {
                matches!(
                    track.mime.split(';').next().unwrap_or(""),
                    "audio/flac" | "audio/x-flac"
                )
            };
            if current.up_next
                || next.up_next
                || !same_album
                || !flac(&current.track)
                || !flac(&next.track)
            {
                return None;
            }
            Some(NextTrack {
                from: current.uid,
                key: next.uid,
                request: self.request(&next.track)?,
                duration: (next.track.seconds > 0.0).then_some(next.track.seconds),
            })
        })();
        self.player.plan_successor(next);
    }

    fn play_current(&mut self, position: f64, paused: bool) {
        self.ending_handled = false;
        self.seeking = None;
        let Some((uid, track)) = self.queue.current_entry().map(|e| (e.uid, e.track.clone()))
        else {
            self.player.stop();
            return;
        };
        match self.request(&track) {
            Some(request) => {
                let duration = (track.seconds > 0.0).then_some(track.seconds);
                self.player
                    .play_track(uid, request, position, duration, paused);
            }
            None => {
                self.player.stop();
                self.notify("This track cannot be played right now");
            }
        }
    }

    fn tracks(&self, indices: &[usize]) -> Vec<Arc<Track>> {
        self.catalog.as_ref().map_or_else(Vec::new, |c| {
            indices.iter().map(|&i| c.track(i).clone()).collect()
        })
    }

    fn next(&mut self, manual: bool) {
        if !manual && self.settings.repeat == Repeat::One && self.queue.current.is_some() {
            return self.play_current(0.0, false);
        }
        match self.queue.advance(self.settings.repeat != Repeat::Off) {
            Some(_) => self.play_current(0.0, false),
            None => self.player.stop(),
        }
    }

    fn previous(&mut self) {
        let position = self.player.output.snapshot().position;
        // Restart the track, or go back one when it has only just started.
        if position > 3.0 || self.queue.current == Some(0) || self.queue.previous().is_some() {
            self.play_current(0.0, false);
        }
    }

    fn toggle_play(&mut self) {
        let snapshot = self.player.output.snapshot();
        match snapshot.phase {
            Phase::Idle | Phase::Ended => {
                if self.queue.current.is_none() && !self.queue.is_empty() {
                    self.queue.jump(0);
                }
                if self.queue.current.is_some() {
                    self.play_current(0.0, false);
                }
            }
            Phase::Failed(_) => self.play_current(snapshot.position, false),
            _ => self.player.output.pause(!snapshot.paused),
        }
    }

    fn apply(&mut self, action: Action) {
        self.follow_gapless();
        if matches!(
            action,
            Action::Play { .. }
                | Action::Enqueue { .. }
                | Action::QueueJump(_)
                | Action::QueueRemove(_)
                | Action::QueueClear
                | Action::Next
                | Action::Previous
                | Action::SetShuffle(_)
        ) {
            self.mark_listening();
        }
        match action {
            Action::Go(page) => self.go(page),
            Action::Back => self.back(),
            Action::Play {
                tracks,
                start,
                shuffle,
            } => {
                if let Some(shuffle) = shuffle {
                    self.settings.shuffle = shuffle;
                }
                self.ordered_album = !self.settings.shuffle
                    && tracks.len() > 1
                    && self.catalog.as_ref().is_some_and(|catalog| {
                        catalog
                            .album_of(catalog.track(tracks[0]))
                            .is_some_and(|album| album.tracks == tracks)
                    });
                let tracks = self.tracks(&tracks);
                if self
                    .queue
                    .replace(tracks, start, self.settings.shuffle)
                    .is_some()
                {
                    self.play_current(0.0, false);
                }
            }
            Action::Enqueue { tracks, next } => {
                let n = tracks.len();
                let was_empty = self.queue.is_empty();
                let tracks = self.tracks(&tracks);
                self.queue.add(tracks, next);
                if was_empty && self.queue.jump(0).is_some() {
                    self.play_current(0.0, false);
                } else {
                    let what = catalog::count(n, "song", "songs");
                    self.notify(if next {
                        format!("{what} will play next")
                    } else {
                        format!("Added {what} to the queue")
                    });
                }
            }
            Action::QueueJump(index) => {
                if self.queue.jump(index).is_some() {
                    self.play_current(0.0, false);
                }
            }
            Action::QueueRemove(index) => {
                if let Removed::Current = self.queue.remove(index) {
                    if self.queue.current.is_some() {
                        self.play_current(0.0, false);
                    } else {
                        self.player.stop();
                    }
                }
            }
            Action::QueueClear => self.queue.clear_upcoming(),
            Action::TogglePlay => self.toggle_play(),
            Action::Next => self.next(true),
            Action::Previous => self.previous(),
            Action::Seek(position) => {
                self.player.seek(position);
                self.seeking = Some((position, Instant::now()));
            }
            Action::SetShuffle(on) => {
                self.settings.shuffle = on;
                self.queue.set_shuffle(on);
            }
            Action::CycleRepeat => {
                self.settings.repeat = match self.settings.repeat {
                    Repeat::Off => Repeat::All,
                    Repeat::All => Repeat::One,
                    Repeat::One => Repeat::Off,
                };
            }
            Action::SetQuality(quality) => {
                if quality != self.settings.quality() {
                    self.settings.quality = quality.value().into();
                    // Restart the current track at the same place in the new quality.
                    let snapshot = self.player.output.snapshot();
                    if matches!(
                        snapshot.phase,
                        Phase::Playing | Phase::Buffering | Phase::Recovering
                    ) {
                        self.play_current(snapshot.position, snapshot.paused);
                    }
                    self.notify(format!("Streaming quality: {}", quality_label(quality)));
                }
            }
            Action::Reload => {
                if self.loading.is_none() {
                    self.notify("Reloading the library…");
                }
                self.load_library(true);
            }
            Action::SignOut => self.sign_out(),
            Action::ToggleFavourite(album) => {
                let count = self.listening.albums.len();
                if let Some(catalog) = &self.catalog
                    && let Some(slot) = catalog.album_index.get(&album)
                {
                    self.listening
                        .albums
                        .retain(|id| catalog.album_index.get(id) != Some(slot));
                }
                if self.listening.albums.len() == count {
                    self.listening.toggle_album(album);
                }
                self.collection_rows = None;
                self.mark_listening();
            }
            Action::ToggleGapless => self.settings.gapless = !self.settings.gapless,
            Action::RecoverOutput => self.recover_output(),
            Action::ToggleQueuePanel => self.settings.queue_panel = !self.settings.queue_panel,
        }
        self.update_gapless();
    }

    fn mark_listening(&mut self) {
        if !self.listening_owner.is_empty() {
            self.listening_dirty.get_or_insert_with(Instant::now);
            self.ctx.request_repaint_after(Duration::from_millis(700));
        }
    }

    fn flush_listening(&mut self) {
        if self.listening_dirty.take().is_some() && !self.listening_owner.is_empty() {
            self.listening.queue = self.queue.saved();
            self.listening_writer
                .save(&self.listening_owner, self.listening.clone());
        }
    }

    fn recover_output(&mut self) {
        let snapshot = self.player.output.snapshot();
        match Output::device() {
            Ok((output, stream)) => match Player::new(output) {
                Ok(player) => {
                    self.player.stop();
                    let old = std::mem::replace(&mut self.player, player);
                    thread::spawn(move || drop(old));
                    self._stream = Some(stream);
                    self.audio_error = None;
                    if self.queue.current_entry().is_some() && snapshot.phase != Phase::Idle {
                        self.play_current(snapshot.position, true);
                    }
                }
                Err(error) => self.audio_error = Some(error.to_string()),
            },
            Err(error) => {
                self.audio_error = Some(format!("No audio output is available ({error})"))
            }
        }
    }

    fn handle_media(&mut self) {
        let events: Vec<_> = self
            .media
            .as_ref()
            .map(|m| m.events.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            use souvlaki::{MediaControlEvent as E, SeekDirection};
            let state = self.player.output.snapshot();
            let seek = |direction, seconds: f64| {
                Action::Seek(
                    (state.position
                        + if matches!(direction, SeekDirection::Forward) {
                            seconds
                        } else {
                            -seconds
                        })
                    .max(0.0),
                )
            };
            match event {
                E::Toggle => self.apply(Action::TogglePlay),
                E::Play
                    if state.paused
                        || matches!(state.phase, Phase::Idle | Phase::Ended | Phase::Failed(_)) =>
                {
                    self.apply(Action::TogglePlay)
                }
                E::Pause => self.player.output.pause(true),
                E::Next => self.apply(Action::Next),
                E::Previous => self.apply(Action::Previous),
                E::Stop => self.player.stop(),
                E::Seek(direction) => self.apply(seek(direction, 10.0)),
                E::SeekBy(direction, offset) => self.apply(seek(direction, offset.as_secs_f64())),
                E::SetPosition(position) => self.apply(Action::Seek(position.0.as_secs_f64())),
                E::Raise => self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus),
                E::SetVolume(gain) if gain.is_finite() => {
                    self.settings.volume = gain.clamp(0.0, 1.0).sqrt() as f32;
                    self.settings.muted = false;
                    #[cfg(target_os = "linux")]
                    if let Some(media) = self.media.as_mut() {
                        media.volume(gain.clamp(0.0, 1.0));
                    }
                }
                _ => {}
            }
        }
    }

    pub fn go(&mut self, page: Page) {
        if page == self.page {
            return;
        }
        if page != Page::Search
            && self.page == Page::Search
            && !matches!(page, Page::Album(_) | Page::Artist(..))
        {
            self.query.clear();
        }
        let previous = std::mem::replace(&mut self.page, page);
        self.history.push(previous);
        if self.history.len() > 64 {
            self.history.remove(0);
        }
    }

    pub fn back(&mut self) {
        if let Some(page) = self.history.pop() {
            if self.page == Page::Search {
                self.query.clear();
            }
            self.page = page;
        }
    }

    fn keyboard(&mut self, ctx: &egui::Context) -> Vec<Action> {
        let mut actions = Vec::new();
        let typing = ctx.egui_wants_keyboard_input();
        ctx.input_mut(|input| {
            let command = egui::Modifiers::COMMAND;
            if input.consume_key(command, Key::F) {
                self.search_focus = true;
            }
            if input.pointer.button_pressed(egui::PointerButton::Extra1) {
                actions.push(Action::Back);
            }
            if typing {
                return;
            }
            if input.consume_key(egui::Modifiers::NONE, Key::Space) {
                actions.push(Action::TogglePlay);
            }
            if input.consume_key(command, Key::ArrowRight) {
                actions.push(Action::Next);
            }
            if input.consume_key(command, Key::ArrowLeft) {
                actions.push(Action::Previous);
            }
            let position = self.position();
            if input.consume_key(egui::Modifiers::NONE, Key::ArrowRight) {
                actions.push(Action::Seek(position + 10.0));
            }
            if input.consume_key(egui::Modifiers::NONE, Key::ArrowLeft) {
                actions.push(Action::Seek((position - 10.0).max(0.0)));
            }
            if input.consume_key(command, Key::ArrowUp) {
                self.settings.volume = (self.settings.volume + 0.05).min(1.0);
                self.settings.muted = false;
            }
            if input.consume_key(command, Key::ArrowDown) {
                self.settings.volume = (self.settings.volume - 0.05).max(0.0);
            }
            if input.consume_key(egui::Modifiers::NONE, Key::Escape) {
                actions.push(Action::Back);
            }
            if input.consume_key(command, Key::R) {
                actions.push(Action::Reload);
            }
        });
        actions
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.flush_listening();
        if let Some(stop) = self.loading.take() {
            stop.cancel();
        }
        if let Stage::SignIn { stop, .. } = &self.stage {
            stop.cancel();
        }
        self.session.close();
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok((generation, message)) = self.receive.try_recv() {
            self.handle(generation, message);
        }
        if !self.media_attempted && matches!(self.stage, Stage::Ready) {
            #[cfg(debug_assertions)]
            if matches!(self.source, Some(Source::Fixture { .. })) {
                self.media_attempted = true;
            }
        }
        if !self.media_attempted && matches!(self.stage, Stage::Ready) {
            self.media = media::Media::new(_frame, ctx.clone());
            self.media_attempted = true;
        }
        self.follow_gapless();
        self.handle_media();
        self.update_gapless();
        if let Some(since) = self.listening_dirty {
            let left = Duration::from_millis(700).saturating_sub(since.elapsed());
            if left.is_zero() {
                self.flush_listening();
            } else {
                ctx.request_repaint_after(left);
            }
        }
        let snapshot = self.player.output.snapshot();
        if snapshot.device_error && self.audio_error.is_none() {
            self.audio_error = Some("The audio output changed or became unavailable".into());
        }
        if snapshot.listened >= 10.0
            && let Some(id) = self.queue.current_entry().map(|entry| entry.track.id)
            && self.listening.record(id)
        {
            self.mark_listening();
            self.collection_rows = None;
        }
        if let Some(media) = &mut self.media {
            let entry = self.queue.current_entry();
            let art = entry.and_then(|entry| {
                self.catalog.as_ref().and_then(|catalog| {
                    catalog
                        .album_of(&entry.track)
                        .and_then(|album| catalog.library.artwork_url(album.artwork, 300))
                })
            });
            media.update(entry, &snapshot, art);
        }
        if snapshot.phase == Phase::Ended && !self.ending_handled {
            self.ending_handled = true;
            self.next(false);
            self.mark_listening();
        }
        if let Some((target, at)) = self.seeking
            && (at.elapsed() > Duration::from_secs(4)
                || snapshot.note.is_some()
                || matches!(snapshot.phase, Phase::Playing if (snapshot.position - target).abs() < 1.5)
                || matches!(snapshot.phase, Phase::Failed(_) | Phase::Idle))
        {
            self.seeking = None;
        }
        let title = match self.queue.current_entry() {
            Some(entry) if snapshot.phase != Phase::Idle => format!(
                "{} · {}",
                entry.track.title,
                catalog::artist_label(&entry.track.artist)
            ),
            _ => APP_NAME.to_owned(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        self.player.output.set_volume(self.settings.gain());
        match snapshot.phase {
            Phase::Buffering | Phase::Recovering => {
                ctx.request_repaint_after(Duration::from_millis(100))
            }
            Phase::Playing if !snapshot.paused => {
                ctx.request_repaint_after(Duration::from_millis(100))
            }
            Phase::Playing => ctx.request_repaint_after(Duration::from_millis(500)),
            _ => {}
        }
        if let Some((_, at)) = &self.toast {
            let left = Duration::from_secs(4).saturating_sub(at.elapsed());
            if left.is_zero() {
                self.toast = None;
            } else {
                ctx.request_repaint_after(left);
            }
        }
        if let Stage::SignIn { busy: true, .. } = self.stage {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.art.begin_frame();
        if let Stage::SignIn { .. } = self.stage {
            if let Some(action) = self.sign_in(ui) {
                match action {
                    SignInAction::Start => self.connect(),
                    SignInAction::Reset => {
                        self.session.close();
                        if let Err(error) = logout(self.client_id.clone()) {
                            self.notify(error.to_string());
                        }
                        self.connect();
                    }
                    SignInAction::Cancel => {
                        self.session.close();
                        self.generation = self.generation.wrapping_add(1);
                        if let Stage::SignIn {
                            stop, busy, code, ..
                        } = &mut self.stage
                        {
                            stop.cancel();
                            *busy = false;
                            *code = None;
                        }
                    }
                }
            }
            return;
        }
        let mut actions = self.keyboard(&ctx);
        egui::Panel::bottom("now-playing")
            .exact_size(86.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::panel(theme::BAR, Margin::symmetric(16, 10)))
            .show(ui, |ui| self.now_playing(ui, &mut actions));
        egui::Panel::left("sidebar")
            .exact_size(228.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::panel(theme::SIDEBAR, Margin::symmetric(12, 14)))
            .show(ui, |ui| self.sidebar(ui, &mut actions));
        if self.settings.queue_panel && self.page != Page::Queue && ui.available_width() >= 680.0 {
            egui::Panel::right("queue-preview")
                .default_size(236.0)
                .resizable(true)
                .frame(theme::panel(theme::SIDEBAR, Margin::symmetric(12, 14)))
                .show(ui, |ui| self.queue_preview(ui, &mut actions));
        }
        egui::CentralPanel::default()
            .frame(theme::panel(
                theme::BG,
                Margin {
                    left: 26,
                    right: 18,
                    top: 18,
                    bottom: 0,
                },
            ))
            .show(ui, |ui| self.main(ui, &mut actions));
        self.toast(&ctx);
        for action in actions {
            self.apply(action);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "settings", &self.settings);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }
}

enum SignInAction {
    Start,
    Cancel,
    Reset,
}

impl App {
    fn sign_in(&mut self, ui: &mut egui::Ui) -> Option<SignInAction> {
        let Stage::SignIn {
            code, error, busy, ..
        } = &self.stage
        else {
            return None;
        };
        let mut action = None;
        egui::CentralPanel::default()
            .frame(theme::panel(theme::BG, 0))
            .show(ui, |ui| {
                let card = egui::vec2(420.0, 380.0);
                let rect = egui::Rect::from_center_size(ui.max_rect().center(), card);
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                    egui::Frame::new()
                        .fill(theme::SURFACE)
                        .corner_radius(14)
                        .inner_margin(28)
                        .stroke(egui::Stroke::new(1.0, theme::LINE))
                        .show(ui, |ui| {
                            ui.set_width(card.x - 56.0);
                            ui.vertical_centered(|ui| {
                                if let Some(logo) = &self.logo {
                                    ui.add(egui::Image::new(logo).fit_to_exact_size(egui::Vec2::splat(64.0)));
                                }
                                ui.add_space(6.0);
                                ui.label(RichText::new(APP_NAME).font(theme::strong(22.0)).color(theme::TEXT));
                                ui.label(RichText::new("Your iBroadcast library, in a light desktop player").color(theme::MUTED));
                                ui.add_space(18.0);
                                match (code, busy) {
                                    (Some((uri, user_code)), true) => {
                                        ui.label(RichText::new("Open iBroadcast and enter this code").color(theme::MUTED));
                                        ui.add_space(6.0);
                                        ui.label(RichText::new(user_code).font(theme::strong(32.0)).color(theme::CREAM));
                                        ui.add_space(12.0);
                                        ui.horizontal(|ui| {
                                            let width = 2.0 * 150.0 + 8.0;
                                            ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
                                            if theme::pill(ui, None, "Open iBroadcast", true).clicked() {
                                                ui.ctx().open_url(egui::OpenUrl::new_tab(uri));
                                            }
                                            if theme::pill(ui, None, "Copy code", false).clicked() {
                                                ui.ctx().copy_text(user_code.clone());
                                            }
                                        });
                                        ui.add_space(14.0);
                                        ui.horizontal(|ui| {
                                            ui.add_space((ui.available_width() - 190.0).max(0.0) / 2.0);
                                            ui.spinner();
                                            ui.label(RichText::new("Waiting for approval…").color(theme::MUTED));
                                        });
                                        ui.add_space(6.0);
                                        if ui.link("Cancel").clicked() {
                                            action = Some(SignInAction::Cancel);
                                        }
                                    }
                                    (None, true) => {
                                        ui.add_space(30.0);
                                        ui.spinner();
                                        ui.label(RichText::new("Connecting to iBroadcast…").color(theme::MUTED));
                                    }
                                    (_, false) => {
                                        if let Some(error) = error {
                                            let error = error.to_string();
                                            ui.label(RichText::new(error).color(theme::ERROR));
                                            ui.add_space(10.0);
                                        } else {
                                            ui.label(RichText::new("Sign in once with a device code. The player only reads your library.").color(theme::MUTED));
                                            ui.add_space(10.0);
                                        }
                                        if theme::pill(ui, None, "Sign in with iBroadcast", true).clicked() {
                                            action = Some(SignInAction::Start);
                                        }
                                        // A stored sign-in that cannot be used; a network problem needs no reset.
                                        if error.as_ref().is_some_and(|e| !e.temporary()) {
                                            ui.add_space(8.0);
                                            if ui.link(RichText::new("Forget the stored sign-in and start over").color(theme::MUTED)).clicked() {
                                                action = Some(SignInAction::Reset);
                                            }
                                        }
                                    }
                                }
                            });
                        });
                });
            });
        action
    }

    fn toast(&mut self, ctx: &egui::Context) {
        let Some((text, at)) = &self.toast else {
            return;
        };
        let fade = (4.0 - at.elapsed().as_secs_f32()).clamp(0.0, 0.4) / 0.4;
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("toast"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -(86.0 + 18.0)])
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_max_width(screen.width() * 0.6);
                egui::Frame::new()
                    .fill(theme::SURFACE_HI.gamma_multiply(fade))
                    .corner_radius(10)
                    .inner_margin(Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).color(theme::TEXT.gamma_multiply(fade)));
                    });
            });
    }
}
