//! Albums, artists and sort orders derived from the iBroadcast library.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use tagcast_player::account::{Library, Track, fold};

pub struct Album {
    pub id: u64,
    pub name: String,
    pub artist: String,
    pub year: u32,
    pub artwork: u64,
    pub tracks: Vec<usize>,
    pub discs: Vec<DiscSection>,
    pub seconds: f64,
    pub search: String,
    sort_artist: String,
    sort_name: String,
    added: u64,
    rating: u8,
}

pub struct DiscSection {
    pub start: usize,
    pub number: u64,
    pub encoded: bool,
}

impl Album {
    fn new(id: u64, name: &str, year: u32, added: u64, rating: u8) -> Self {
        Self {
            id,
            name: if name.trim().is_empty() {
                "Unknown album".into()
            } else {
                name.to_owned()
            },
            artist: String::new(),
            year,
            artwork: 0,
            tracks: Vec::new(),
            discs: Vec::new(),
            seconds: 0.0,
            search: String::new(),
            sort_artist: String::new(),
            sort_name: String::new(),
            added,
            rating,
        }
    }

    pub fn sort_detail(&self, sort: AlbumSort) -> Option<String> {
        match sort {
            AlbumSort::Added if self.added > 0 => {
                let date = self.added / 1_000_000;
                Some(format!(
                    "Added {:04}-{:02}-{:02}",
                    date / 10_000,
                    date / 100 % 100,
                    date % 100
                ))
            }
            AlbumSort::Rating if self.rating > 0 => Some(format!("Rating {}/5", self.rating)),
            _ => None,
        }
    }
}

pub struct Artist {
    pub name: String,
    pub key: String,
    credits: [ArtistCredits; 2],
    pub search: String,
}

#[derive(Default)]
pub struct ArtistCredits {
    pub albums: Vec<usize>,
    pub tracks: Vec<usize>,
    pub artwork: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArtistRole {
    Album,
    Track,
}

impl ArtistRole {
    pub const ALL: [Self; 2] = [Self::Album, Self::Track];

    pub fn label(self) -> &'static str {
        match self {
            Self::Album => "Album artists",
            Self::Track => "Track artists",
        }
    }

    pub fn singular(self) -> &'static str {
        match self {
            Self::Album => "Album artist",
            Self::Track => "Track artist",
        }
    }
}

impl Artist {
    pub fn credits(&self, role: ArtistRole) -> Option<&ArtistCredits> {
        let credits = &self.credits[role as usize];
        (!credits.albums.is_empty()).then_some(credits)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ArtistSort {
    #[default]
    Name,
    ReverseName,
    Random,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum AlbumSort {
    Artist,
    Title,
    #[serde(alias = "Newest")]
    Year,
    Added,
    Rating,
    Random,
}

impl AlbumSort {
    pub fn descending(self, reversed: bool) -> bool {
        matches!(self, Self::Year | Self::Added | Self::Rating) ^ reversed
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Artist => "Artist",
            Self::Title => "Title",
            Self::Year => "Year",
            Self::Added => "Added",
            Self::Rating => "Rating",
            Self::Random => "Random",
        }
    }
}

fn compare_known<T: Ord + Default + PartialEq>(a: T, b: T, descending: bool) -> Ordering {
    (a == T::default())
        .cmp(&(b == T::default()))
        .then_with(|| directed(a.cmp(&b), descending))
}

fn directed(order: Ordering, descending: bool) -> Ordering {
    if descending { order.reverse() } else { order }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum BrowseKind {
    #[default]
    Genres,
    Tags,
    Years,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum BrowseGroup {
    Genre(String),
    Tag(String),
    Year(u32),
}

impl BrowseKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Genres => "Genres",
            Self::Tags => "Tags",
            Self::Years => "Years",
        }
    }
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub enum BrowseSort {
    #[default]
    Tracks,
    Albums,
    Ascending,
    Descending,
}

pub struct GroupSummary {
    pub name: String,
    pub albums: usize,
    pub tracks: usize,
    year: u32,
    search: String,
}

impl GroupSummary {
    fn new(name: String, albums: usize, tracks: usize, year: u32) -> Self {
        Self {
            search: fold(&name),
            name,
            albums,
            tracks,
            year,
        }
    }

    pub fn order(groups: &[Self], query: &str, sort: BrowseSort) -> Vec<usize> {
        let folded = fold(query);
        let words: Vec<_> = folded.split_whitespace().collect();
        let mut order: Vec<_> = groups
            .iter()
            .enumerate()
            .filter_map(|(i, group)| {
                words
                    .iter()
                    .all(|word| group.search.contains(word))
                    .then_some(i)
            })
            .collect();
        order.sort_unstable_by(|&left, &right| {
            let (a, b) = (&groups[left], &groups[right]);
            let by_name = || (a.year, &a.search, &a.name).cmp(&(b.year, &b.search, &b.name));
            match sort {
                BrowseSort::Tracks => b.tracks.cmp(&a.tracks).then_with(by_name),
                BrowseSort::Albums => b.albums.cmp(&a.albums).then_with(by_name),
                BrowseSort::Ascending => by_name(),
                BrowseSort::Descending => by_name().reverse(),
            }
        });
        order
    }

    pub fn year(&self) -> u32 {
        self.year
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SongSort {
    Artist,
    Title,
    Album,
    Year,
    Time,
}

pub struct Catalog {
    pub library: Library,
    pub by_id: HashMap<u64, usize>,
    playlists_by_id: HashMap<u64, usize>,
    groups: HashMap<BrowseKind, Vec<GroupSummary>>,
    pub albums: Vec<Album>,
    pub album_index: HashMap<u64, usize>,
    set_by_track: HashMap<u64, usize>,
    pub artists: Vec<Artist>,
    artist_index: HashMap<String, usize>,
    artist_counts: [usize; 2],
    pub total_seconds: f64,
    album_orders: HashMap<(AlbumSort, bool), Arc<Vec<usize>>>,
    song_orders: HashMap<(SongSort, bool), Arc<Vec<usize>>>,
}

pub const UNKNOWN_ARTIST: &str = "Unknown artist";

pub fn artist_label(name: &str) -> &str {
    if name.trim().is_empty() {
        UNKNOWN_ARTIST
    } else {
        name
    }
}

impl Catalog {
    pub fn new(library: Library) -> Self {
        let by_id: HashMap<_, _> = library
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id, i))
            .collect();

        let mut albums = Vec::new();
        let mut album_index = HashMap::new();
        let mut set_by_track = HashMap::new();
        let mut aliases = HashMap::<u64, Option<usize>>::new();
        for (&id, set) in &library.album_sets {
            let slot = albums.len();
            let mut album = Album::new(id, &set.name, set.year, set.added, set.rating);
            album.artist.clone_from(&set.artist);
            for disc in &set.discs {
                let start = album.tracks.len();
                for &track_id in &disc.tracks {
                    let Some(&index) = by_id.get(&track_id) else {
                        continue;
                    };
                    if set_by_track.contains_key(&track_id) {
                        continue;
                    }
                    let track = &library.tracks[index];
                    album.tracks.push(index);
                    album.seconds += track.seconds;
                    set_by_track.insert(track_id, slot);
                    let alias = aliases.entry(track.album_id).or_insert(Some(slot));
                    if *alias != Some(slot) {
                        *alias = None;
                    }
                }
                if album.tracks.len() > start {
                    album.discs.push(DiscSection {
                        start,
                        number: disc.number,
                        encoded: disc.encoded,
                    });
                }
            }
            if !album.tracks.is_empty() {
                album_index.insert(id, slot);
                albums.push(album);
            }
        }

        // Tracks outside a projected set keep their original album. Only use
        // a source ID as an alias when all of its tracks resolve to one set.
        for (index, track) in library.tracks.iter().enumerate() {
            if set_by_track.contains_key(&track.id) {
                continue;
            }
            aliases.remove(&track.album_id);
            let slot = *album_index.entry(track.album_id).or_insert_with(|| {
                let meta = library.album_metadata.get(&track.album_id);
                albums.push(Album::new(
                    track.album_id,
                    &track.album,
                    0,
                    meta.map_or(0, |meta| meta.added),
                    meta.map_or(0, |meta| meta.rating),
                ));
                albums.len() - 1
            });
            let album = &mut albums[slot];
            album.tracks.push(index);
            album.seconds += track.seconds;
            album.year = album.year.max(track.year);
        }
        for id in &library.favourite_albums {
            if let Some(&index) = album_index.get(id)
                && albums[index].discs.is_empty()
            {
                albums[index].rating = 5;
            }
        }
        for (source, slot) in aliases {
            if let Some(slot) = slot {
                album_index.entry(source).or_insert(slot);
            }
        }
        for album in &mut albums {
            let tracks = &library.tracks;
            if album.discs.is_empty() {
                album
                    .tracks
                    .sort_by_key(|&i| (tracks[i].disc, tracks[i].number, tracks[i].id));
                let first = &tracks[album.tracks[0]];
                album.artist = if !first.album_artist.trim().is_empty() {
                    first.album_artist.clone()
                } else if album
                    .tracks
                    .iter()
                    .all(|&i| fold(&tracks[i].artist) == fold(&first.artist))
                {
                    artist_label(&first.artist).to_owned()
                } else {
                    "Various Artists".into()
                };
                album.artwork = album
                    .tracks
                    .iter()
                    .map(|&i| &tracks[i])
                    .filter(|t| t.artwork > 0)
                    .min_by_key(|t| (if t.number == 0 { u64::MAX } else { t.number }, t.id))
                    .map_or(0, |t| t.artwork);
            } else {
                album.artwork = album
                    .tracks
                    .iter()
                    .map(|&i| tracks[i].artwork)
                    .find(|&art| art > 0)
                    .unwrap_or(0);
            }
            if let Some(&id) = library
                .album_artist_ids
                .get(&album.id)
                .and_then(|ids| ids.first())
            {
                album.artist = library.artist_name(id).to_owned();
            }
            album.sort_artist = sort_key(&album.artist);
            album.sort_name = sort_key(&album.name);
            album.search = fold(&format!("{} {} {}", album.name, album.artist, album.year));
        }

        // One name and portrait, with independent credits for both views.
        let mut artists: Vec<Artist> = Vec::new();
        let mut artist_index: HashMap<String, usize> = HashMap::new();
        let mut add =
            |name: &str, role: ArtistRole, tracks: &[usize], album: usize, artwork: u64| {
                let name = artist_label(name);
                let key = fold(name.trim());
                let slot = *artist_index.entry(key.clone()).or_insert_with(|| {
                    artists.push(Artist {
                        name: name.trim().to_owned(),
                        key: key.clone(),
                        credits: Default::default(),
                        search: key,
                    });
                    artists.len() - 1
                });
                let credits = &mut artists[slot].credits[role as usize];
                let first_for_album = credits.albums.last() != Some(&album);
                if first_for_album {
                    credits.albums.push(album);
                    if credits.artwork == 0 {
                        credits.artwork = artwork;
                    }
                }
                if role == ArtistRole::Album {
                    if first_for_album {
                        credits.tracks.extend_from_slice(tracks);
                    }
                } else if credits.tracks.last() != tracks.first() {
                    credits.tracks.extend_from_slice(tracks);
                }
            };
        for (slot, album) in albums.iter().enumerate() {
            add(
                &album.artist,
                ArtistRole::Album,
                &album.tracks,
                slot,
                album.artwork,
            );
            if let Some(ids) = library.album_artist_ids.get(&album.id) {
                for &id in ids.iter().skip(1) {
                    add(
                        library.artist_name(id),
                        ArtistRole::Album,
                        &album.tracks,
                        slot,
                        album.artwork,
                    );
                }
            }
            for &index in &album.tracks {
                let track = &library.tracks[index];
                let track_row = std::slice::from_ref(&index);
                add(
                    &track.artist,
                    ArtistRole::Track,
                    track_row,
                    slot,
                    album.artwork,
                );
                if let Some(ids) = library.extra_performer_ids.get(&track.id) {
                    for &id in ids {
                        add(
                            library.artist_name(id),
                            ArtistRole::Track,
                            track_row,
                            slot,
                            album.artwork,
                        );
                    }
                }
            }
        }
        artists.sort_by_cached_key(|a| sort_key(&a.name));
        let artist_index = artists
            .iter()
            .enumerate()
            .map(|(i, a)| (a.key.clone(), i))
            .collect();
        let mut artist_counts = [0; 2];
        for artist in &mut artists {
            for (role, credits) in artist.credits.iter_mut().enumerate() {
                if !credits.albums.is_empty() {
                    artist_counts[role] += 1;
                }
                credits.albums.sort_by(|&a, &b| {
                    let (a, b) = (&albums[a], &albums[b]);
                    b.year
                        .cmp(&a.year)
                        .then_with(|| a.sort_name.cmp(&b.sort_name))
                });
            }
        }
        let total_seconds = library.tracks.iter().map(|t| t.seconds).sum();

        let playlists_by_id = library
            .playlists
            .iter()
            .enumerate()
            .map(|(index, list)| (list.id, index))
            .collect();
        Self {
            library,
            by_id,
            playlists_by_id,
            groups: HashMap::new(),
            albums,
            album_index,
            set_by_track,
            artists,
            artist_index,
            artist_counts,
            total_seconds,
            album_orders: HashMap::new(),
            song_orders: HashMap::new(),
        }
    }

    pub fn playlist_index(&self, id: u64) -> Option<usize> {
        self.playlists_by_id.get(&id).copied()
    }

    pub fn groups(&mut self, kind: BrowseKind) -> &[GroupSummary] {
        let album_index = &self.album_index;
        let set_by_track = &self.set_by_track;
        let catalog_albums = &self.albums;
        let slot = |track: &Track| {
            set_by_track
                .get(&track.id)
                .or_else(|| album_index.get(&track.album_id))
                .copied()
        };
        self.groups.entry(kind).or_insert_with(|| {
            if kind == BrowseKind::Years {
                let mut years = BTreeMap::<u32, (usize, HashSet<usize>)>::new();
                for track in &self.library.tracks {
                    let Some(album) = slot(track) else { continue };
                    let year = if track.year > 0 {
                        track.year
                    } else {
                        catalog_albums[album].year
                    };
                    if year == 0 {
                        continue;
                    }
                    let (tracks, albums) = years.entry(year).or_default();
                    *tracks += 1;
                    albums.insert(album);
                }
                return years
                    .into_iter()
                    .map(|(year, (tracks, albums))| {
                        GroupSummary::new(year.to_string(), albums.len(), tracks, year)
                    })
                    .collect();
            }
            let groups = if kind == BrowseKind::Genres {
                &self.library.genres
            } else {
                &self.library.tags
            };
            let mut tracks = HashSet::new();
            let mut albums = HashSet::new();
            groups
                .iter()
                .filter_map(|(name, ids)| {
                    tracks.clear();
                    albums.clear();
                    for id in ids {
                        if let Some(&index) = self.by_id.get(id) {
                            tracks.insert(id);
                            albums.extend(slot(&self.library.tracks[index]));
                        }
                    }
                    (!tracks.is_empty())
                        .then(|| GroupSummary::new(name.clone(), albums.len(), tracks.len(), 0))
                })
                .collect()
        })
    }

    pub fn group_album_slots(&self, group: &BrowseGroup) -> HashSet<usize> {
        let ids = match group {
            BrowseGroup::Genre(label) => self.library.genres.get(label),
            BrowseGroup::Tag(label) => self.library.tags.get(label),
            BrowseGroup::Year(year) => {
                return self
                    .library
                    .tracks
                    .iter()
                    .filter_map(|track| {
                        let album = self.album_slot(track)?;
                        let release_year = if track.year > 0 {
                            track.year
                        } else {
                            self.albums[album].year
                        };
                        (release_year == *year).then_some(album)
                    })
                    .collect();
            }
        };
        ids.into_iter()
            .flatten()
            .filter_map(|&id| {
                self.track_index(id)
                    .and_then(|index| self.album_slot(self.track(index)))
            })
            .collect()
    }

    pub fn track_index(&self, id: u64) -> Option<usize> {
        self.by_id.get(&id).copied()
    }

    pub fn track(&self, index: usize) -> &Arc<Track> {
        &self.library.tracks[index]
    }

    pub fn album_of(&self, track: &Track) -> Option<&Album> {
        self.album_slot(track).map(|i| &self.albums[i])
    }

    pub fn album_slot(&self, track: &Track) -> Option<usize> {
        self.set_by_track
            .get(&track.id)
            .or_else(|| self.album_index.get(&track.album_id))
            .copied()
    }

    pub fn artist_by_name(&self, name: &str, role: ArtistRole) -> Option<&Artist> {
        self.artist_by_key(&fold(artist_label(name).trim()), role)
    }

    pub fn artist_by_key(&self, key: &str, role: ArtistRole) -> Option<&Artist> {
        let artist = &self.artists[*self.artist_index.get(key)?];
        artist.credits(role).map(|_| artist)
    }

    pub fn artist_count(&self, role: ArtistRole) -> usize {
        self.artist_counts[role as usize]
    }

    pub fn playlist_tracks(&self, playlist: usize) -> Vec<usize> {
        self.library
            .playlists
            .get(playlist)
            .map(|p| {
                p.tracks
                    .iter()
                    .filter_map(|id| self.by_id.get(id).copied())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn reshuffle_albums(&mut self) {
        let previous = self.album_orders.remove(&(AlbumSort::Random, false));
        let mut order: Vec<_> = (0..self.albums.len()).collect();
        fastrand::shuffle(&mut order);
        if order.len() > 1 && previous.as_deref() == Some(&order) {
            order.swap(0, 1);
        }
        self.album_orders
            .insert((AlbumSort::Random, false), Arc::new(order));
    }

    pub fn album_order(&mut self, sort: AlbumSort, reversed: bool) -> Arc<Vec<usize>> {
        let albums = &self.albums;
        let descending = sort.descending(reversed);
        self.album_orders
            .entry((sort, reversed))
            .or_insert_with(|| {
                let mut order: Vec<usize> = (0..albums.len()).collect();
                if sort == AlbumSort::Random {
                    fastrand::shuffle(&mut order);
                } else {
                    order.sort_unstable_by(|&a, &b| {
                        let (a, b) = (&albums[a], &albums[b]);
                        let primary = match sort {
                            AlbumSort::Artist => {
                                directed(a.sort_artist.cmp(&b.sort_artist), descending)
                            }
                            AlbumSort::Title => directed(a.sort_name.cmp(&b.sort_name), descending),
                            AlbumSort::Year => compare_known(a.year, b.year, descending),
                            AlbumSort::Added => compare_known(a.added, b.added, descending),
                            AlbumSort::Rating => compare_known(a.rating, b.rating, descending),
                            AlbumSort::Random => unreachable!(),
                        };
                        let tie = if sort == AlbumSort::Title {
                            a.sort_artist.cmp(&b.sort_artist)
                        } else {
                            a.sort_name.cmp(&b.sort_name)
                        };
                        primary.then(tie).then_with(|| a.id.cmp(&b.id))
                    });
                }
                Arc::new(order)
            })
            .clone()
    }

    pub fn song_order(&mut self, sort: SongSort, descending: bool) -> Arc<Vec<usize>> {
        if let Some(order) = self.song_orders.get(&(sort, descending)) {
            return order.clone();
        }
        let tracks = &self.library.tracks;
        let mut order: Vec<usize> = (0..tracks.len()).collect();
        match sort {
            // The library is already in artist, album, disc and track order.
            SongSort::Artist => {}
            SongSort::Title => order.sort_by_cached_key(|&i| sort_key(&tracks[i].title)),
            SongSort::Album => order.sort_by_cached_key(|&i| {
                let t = &tracks[i];
                (sort_key(&t.album), t.disc, t.number)
            }),
            SongSort::Year => order.sort_by_key(|&i| tracks[i].year),
            SongSort::Time => {
                order.sort_by(|&a, &b| tracks[a].seconds.total_cmp(&tracks[b].seconds))
            }
        }
        if descending {
            order.reverse();
        }
        let order = Arc::new(order);
        self.song_orders.insert((sort, descending), order.clone());
        order
    }

    pub fn artist_order(
        &self,
        role: ArtistRole,
        query: &str,
        sort: ArtistSort,
        previous: Option<&[usize]>,
    ) -> Vec<usize> {
        let query = fold(query);
        let words: Vec<_> = query.split_whitespace().collect();
        let mut order: Vec<_> = self
            .artists
            .iter()
            .enumerate()
            .filter_map(|(i, artist)| {
                (artist.credits(role).is_some()
                    && words.iter().all(|word| artist.search.contains(word)))
                .then_some(i)
            })
            .collect();
        match sort {
            ArtistSort::Name => {}
            ArtistSort::ReverseName => order.reverse(),
            ArtistSort::Random => {
                fastrand::shuffle(&mut order);
                if order.len() > 1 && previous == Some(order.as_slice()) {
                    order.swap(0, 1);
                }
            }
        }
        order
    }

    pub fn search(&self, query: &str) -> SearchResults {
        let words: Vec<String> = fold(query.trim())
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let matches = |text: &str| words.iter().all(|w| text.contains(w.as_str()));
        if words.is_empty() {
            return SearchResults::default();
        }
        let mut artists: Vec<usize> = (0..self.artists.len())
            .filter(|&i| matches(&self.artists[i].search))
            .collect();
        artists.sort_by_key(|&i| {
            std::cmp::Reverse(
                self.artists[i]
                    .credits
                    .iter()
                    .map(|credits| credits.tracks.len())
                    .max()
                    .unwrap_or(0),
            )
        });
        artists.truncate(24);
        let albums: Vec<usize> = (0..self.albums.len())
            .filter(|&i| matches(&self.albums[i].search))
            .take(60)
            .collect();
        let tracks: Vec<usize> = self
            .library
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| matches(&t.search))
            .map(|(i, _)| i)
            .take(SEARCH_SONGS)
            .collect();
        SearchResults {
            artists,
            albums,
            tracks,
        }
    }
}

/// Search shows at most this many songs.
pub const SEARCH_SONGS: usize = 500;

#[derive(Default)]
pub struct SearchResults {
    pub artists: Vec<usize>,
    pub albums: Vec<usize>,
    pub tracks: Vec<usize>,
}

/// Case- and accent-insensitive order that ignores a leading "The".
fn sort_key(text: &str) -> String {
    let folded = fold(text.trim());
    folded
        .strip_prefix("the ")
        .map(str::to_owned)
        .unwrap_or(folded)
}

pub fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn format_length(seconds: f64) -> String {
    let minutes = (seconds / 60.0).round() as u64;
    if minutes >= 60 {
        format!("{} hr {} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}

pub fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{} {many}", group_digits(n))
    }
}

fn group_digits(n: usize) -> String {
    let text = n.to_string();
    let mut out = String::new();
    for (i, ch) in text.chars().enumerate() {
        if i > 0 && (text.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn two_discs() -> serde_json::Value {
        json!({
            "library": {
                "artists": {"1": {"name":"An Artist"}},
                "albums": {
                    "10": {"name":"One Album", "artist_id":1, "year":1998, "disc":1,
                           "rating":4, "tracks":[101,102], "artists_additional":[{"artist_id":2}]},
                    "20": {"name":"one album", "artist_id":1, "year":1998, "disc":2,
                           "rating":5, "tracks":[201,202], "artists_additional":[{"artist_id":2}]}
                },
                "tracks": {
                    "101": {"title":"One", "artist_id":1, "album_id":10, "track":1, "file":"/128/a/101",
                            "uploaded_on":"2020-06-01", "uploaded_time":"12:30:00", "genre":"Rock"},
                    "102": {"title":"Two", "artist_id":1, "album_id":10, "track":2, "file":"/128/a/102",
                            "uploaded_on":"2020-06-01", "artwork_id":12},
                    "201": {"title":"Three", "artist_id":1, "album_id":20, "track":1, "file":"/128/a/201",
                            "uploaded_on":"2020-06-01", "artwork_id":21, "genre":"Rock"},
                    "202": {"title":"Four", "artist_id":1, "album_id":20, "track":2, "file":"/128/a/202",
                            "uploaded_on":"2020-06-01"}
                },
                "tags": {"1":{"name":"Both", "tracks":[101,202]}}
            }
        })
    }

    fn track_ids(catalog: &Catalog, album: &Album) -> Vec<u64> {
        album
            .tracks
            .iter()
            .map(|&index| catalog.track(index).id)
            .collect()
    }

    #[test]
    fn local_discs_share_album_navigation_counts_art_and_ordered_queue() {
        let mut catalog = Catalog::new(Library::parse(&two_discs()).unwrap());
        assert_eq!(catalog.albums.len(), 1);
        assert_eq!(catalog.album_index[&10], catalog.album_index[&20]);
        let album = &catalog.albums[0];
        assert_eq!(
            (album.id, album.artwork, album.rating, album.year),
            (10, 12, 4, 1998)
        );
        assert_eq!(
            album
                .discs
                .iter()
                .map(|d| (d.number, d.start))
                .collect::<Vec<_>>(),
            [(1, 0), (2, 2)]
        );
        assert_eq!(track_ids(&catalog, album), [101, 102, 201, 202]);
        let mut queue = crate::gui::queue::Queue::default();
        queue.replace(
            album
                .tracks
                .iter()
                .map(|&i| catalog.track(i).clone())
                .collect(),
            Some(0),
            false,
        );
        assert_eq!(
            queue
                .items
                .iter()
                .map(|entry| entry.track.id)
                .collect::<Vec<_>>(),
            [101, 102, 201, 202]
        );
        let before = catalog.track(catalog.track_index(102).unwrap());
        let after = catalog.track(catalog.track_index(201).unwrap());
        assert_ne!(before.album_id, after.album_id);
        assert_eq!(catalog.album_slot(before), catalog.album_slot(after));
        assert_eq!(catalog.search("one album").albums.len(), 1);
        for kind in [BrowseKind::Genres, BrowseKind::Tags, BrowseKind::Years] {
            assert_eq!(catalog.groups(kind)[0].albums, 1);
        }
        assert_eq!(
            catalog.group_album_slots(&BrowseGroup::Tag("Both".into())),
            HashSet::from([0])
        );
    }

    #[test]
    fn local_grouping_keeps_conflicting_editions_and_duplicate_disc_numbers_separate() {
        for (field, value) in [
            ("year", json!(2001)),
            ("disc", json!(1)),
            ("artist_id", json!(345)),
            ("artists_additional", json!([{"artist_id":50}])),
        ] {
            let mut raw = two_discs();
            raw["library"]["albums"]["20"][field] = value;
            assert_eq!(
                Catalog::new(Library::parse(&raw).unwrap()).albums.len(),
                2,
                "{field}"
            );
        }
        let mut raw = two_discs();
        raw["library"]["tracks"]["202"]["uploaded_on"] = json!("2020-06-02");
        assert_eq!(Catalog::new(Library::parse(&raw).unwrap()).albums.len(), 2);
    }

    #[test]
    fn native_container_keeps_one_cover_with_or_without_source_rows() {
        for keep_sources in [true, false] {
            let mut raw = two_discs();
            raw["combine_sets"] = json!(true);
            raw["library"]["albums"]["90"] = json!({
                "name":"Combined album", "artist_id":1, "year":1998,
                "rating":3, "tracks":[201,202,101,102],
            });
            if !keep_sources {
                let albums = raw["library"]["albums"].as_object_mut().unwrap();
                albums.remove("10");
                albums.remove("20");
                for (id, position) in [
                    ("101", "1-01"),
                    ("102", "1-02"),
                    ("201", "2-01"),
                    ("202", "2-02"),
                ] {
                    raw["library"]["tracks"][id]["track"] = json!(position);
                }
            }
            let library = Library::parse(&raw).unwrap();
            let mut library: Library =
                serde_json::from_value(serde_json::to_value(library).unwrap()).unwrap();
            library.prepare();
            let mut catalog = Catalog::new(library);
            assert_eq!(catalog.albums.len(), 1);
            let album = &catalog.albums[0];
            assert_eq!(
                (album.id, album.name.as_str(), album.rating),
                (90, "Combined album", 3)
            );
            assert_eq!(
                album
                    .discs
                    .iter()
                    .map(|d| (d.number, d.start))
                    .collect::<Vec<_>>(),
                [(1, 0), (2, 2)]
            );
            assert_eq!(track_ids(&catalog, album), [101, 102, 201, 202]);
            assert_eq!(catalog.album_index[&20], 0);
            assert_eq!(catalog.groups(BrowseKind::Years)[0].tracks, 4);
        }
    }

    #[test]
    fn numeric_positions_require_complete_blocks_and_the_native_setting() {
        let mut raw = two_discs();
        raw["library"]["albums"] = json!({
            "90":{"name":"Combined album", "artist_id":1, "tracks":[202,101,201,102], "year":1998}
        });
        for id in ["101", "102", "201", "202"] {
            raw["library"]["tracks"][id]["album_id"] = json!(90);
            raw["library"]["tracks"][id]["track"] = json!(id.parse::<u64>().unwrap());
        }
        let parse = |value: &serde_json::Value| Catalog::new(Library::parse(value).unwrap());
        assert!(parse(&raw).albums[0].discs.is_empty());
        raw["combine_sets"] = json!(true);
        let catalog = parse(&raw);
        let album = &catalog.albums[0];
        assert_eq!(album.discs.len(), 2);
        assert!(album.discs.iter().all(|d| d.encoded));
        assert_eq!(track_ids(&catalog, album), [101, 102, 201, 202]);
        assert_eq!(catalog.track(album.tracks[2]).number, 201);
        for invalid in [json!(203), json!(201), json!("202")] {
            raw["library"]["tracks"]["202"]["track"] = invalid;
            assert!(parse(&raw).albums[0].discs.is_empty());
        }
    }

    #[test]
    fn native_containers_with_ambiguous_membership_preserve_all_source_tracks() {
        let mut raw = two_discs();
        raw["combine_sets"] = json!(true);
        raw["library"]["albums"]["90"] = json!({"name":"Parent", "tracks":[101,102,201,202]});
        raw["library"]["albums"]["91"] = json!({"name":"Other parent", "tracks":[201]});
        let catalog = Catalog::new(Library::parse(&raw).unwrap());
        assert_eq!(catalog.albums.len(), 2);
        assert_eq!(
            catalog
                .albums
                .iter()
                .map(|album| album.tracks.len())
                .sum::<usize>(),
            4
        );
    }

    fn sample() -> Catalog {
        let library = Library::parse(&json!({
            "library": {
                "artists": {"map": {"name": 0}, "1": ["The Cure"], "2": ["Björk"], "3": ["Guest"]},
                "albums": {"map": {"name": 0, "artist_id": 1, "year": 2, "rating": 3},
                           "10": ["Disintegration", 1, 1989, 3], "20": ["Debut", 2, 1993, 5]},
                "tracks": {
                    "map": {"title": 0, "artist_id": 1, "album_id": 2, "file": 3, "track": 4, "artwork_id": 5, "length": 6, "genre": 7, "genres_additional": 8, "uploaded_on": 9},
                    "1": ["Plainsong", 1, 10, "/128/a/1", 1, 0, 312, "Rock", ["Rock", "Pop;Rock"], "2018-04-01"],
                    "2": ["Pictures of You", 1, 10, "/128/a/2", 2, 55, 448, "Rock", [], "2022-04-01"],
                    "3": ["Human Behaviour", 2, 20, "/128/a/3", 1, 66, 252, "Pop", ["Rock"], "2019-10-01"],
                    "4": ["Venus as a Boy", 3, 20, "/128/a/4", 3, 0, 281, "Électro Pop", [], "2019-10-01"]
                },
                "tags": {"1": {"name": "Favorites", "tracks": [1, 1, 3, 999]}}
            }
        }))
        .unwrap();
        Catalog::new(library)
    }

    #[test]
    fn groups_albums_with_cover_year_and_album_artist() {
        let catalog = sample();
        assert_eq!(catalog.albums.len(), 2);
        let cure = &catalog.albums[catalog.album_index[&10]];
        assert_eq!(cure.artist, "The Cure");
        assert_eq!((cure.year, cure.artwork, cure.tracks.len()), (1989, 55, 2));
        let debut = &catalog.albums[catalog.album_index[&20]];
        assert_eq!(debut.artist, "Björk");
        assert_eq!(format_time(cure.seconds), "12:40");
    }

    #[test]
    fn artists_include_guests_and_ignore_a_leading_the_when_sorting() {
        let mut catalog = sample();
        let names: Vec<&str> = catalog.artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Björk", "The Cure", "Guest"]);
        let guest = catalog.artist_by_name("guest", ArtistRole::Track).unwrap();
        assert_eq!(guest.credits(ArtistRole::Track).unwrap().albums.len(), 1);
        assert!(catalog.artist_by_name("guest", ArtistRole::Album).is_none());
        let by_title = catalog.song_order(SongSort::Title, false);
        assert_eq!(catalog.track(by_title[0]).title, "Human Behaviour");
        let newest = catalog.album_order(AlbumSort::Year, false);
        assert_eq!(catalog.albums[newest[0]].name, "Debut");
    }

    #[test]
    fn album_sort_keeps_release_year_distinct_from_upload_and_reverses() {
        let mut catalog = sample();
        for (sort, first) in [
            (AlbumSort::Artist, 20),
            (AlbumSort::Title, 20),
            (AlbumSort::Year, 20),
            (AlbumSort::Added, 10),
            (AlbumSort::Rating, 20),
        ] {
            let forward = catalog.album_order(sort, false);
            let reverse = catalog.album_order(sort, true);
            assert_eq!(catalog.albums[forward[0]].id, first);
            assert_eq!(forward[0], reverse[1]);
        }
        let cure = &catalog.albums[catalog.album_index[&10]];
        assert_eq!(
            cure.sort_detail(AlbumSort::Added).as_deref(),
            Some("Added 2022-04-01")
        );
        assert_eq!(
            cure.sort_detail(AlbumSort::Rating).as_deref(),
            Some("Rating 3/5")
        );
        let mut missing_year = sample();
        let index = missing_year.album_index[&20];
        missing_year.albums[index].year = 0;
        for reversed in [true, false] {
            assert_eq!(
                missing_year.album_order(AlbumSort::Year, reversed)[1],
                index
            );
        }
        let old: AlbumSort = serde_json::from_str("\"Newest\"").unwrap();
        assert_eq!(old, AlbumSort::Year);
        assert!(old.descending(false));
    }

    #[test]
    fn random_album_order_stays_put_until_explicit_reshuffle() {
        let mut catalog = sample();
        let first = catalog.album_order(AlbumSort::Random, false);
        assert!(Arc::ptr_eq(
            &first,
            &catalog.album_order(AlbumSort::Random, false)
        ));
        catalog.reshuffle_albums();
        let next = catalog.album_order(AlbumSort::Random, false);
        assert_ne!(first, next);
        assert_eq!(
            next.iter().copied().collect::<HashSet<_>>(),
            HashSet::from([0, 1])
        );
    }

    #[test]
    fn browse_cards_count_distinct_active_members_and_preserve_labels() {
        let mut catalog = sample();
        let genres = catalog.groups(BrowseKind::Genres);
        let by_tracks = GroupSummary::order(genres, "", BrowseSort::Tracks);
        let rock = &genres[by_tracks[0]];
        assert_eq!(
            (rock.name.as_str(), rock.albums, rock.tracks),
            ("Rock", 2, 3)
        );
        assert!(genres.iter().any(|group| group.name == "Pop;Rock"));
        let matched = GroupSummary::order(genres, "electro pop", BrowseSort::Ascending);
        assert_eq!(matched.len(), 1);
        assert_eq!(genres[matched[0]].name, "Électro Pop");
        let tag = &catalog.groups(BrowseKind::Tags)[0];
        assert_eq!((tag.albums, tag.tracks), (2, 2));
        let years = catalog.groups(BrowseKind::Years);
        let by_year = GroupSummary::order(years, "", BrowseSort::Descending);
        let latest = &years[by_year[0]];
        assert_eq!((latest.year(), latest.albums, latest.tracks), (1993, 1, 2));
    }

    #[test]
    fn browse_groups_resolve_to_unique_albums_and_open_the_complete_album() {
        let catalog = sample();
        let cure = catalog.album_index[&10];
        let debut = catalog.album_index[&20];
        assert_eq!(
            catalog.group_album_slots(&BrowseGroup::Genre("Pop;Rock".into())),
            HashSet::from([cure]),
        );
        assert_eq!(catalog.albums[cure].tracks.len(), 2);
        assert_eq!(
            catalog.group_album_slots(&BrowseGroup::Tag("Favorites".into())),
            HashSet::from([cure, debut]),
        );
        assert_eq!(
            catalog.group_album_slots(&BrowseGroup::Year(1993)),
            HashSet::from([debut])
        );
        assert!(
            catalog
                .group_album_slots(&BrowseGroup::Tag("missing".into()))
                .is_empty()
        );
    }

    #[test]
    fn search_matches_every_word_without_accents() {
        let catalog = sample();
        // The album artist is searchable too.
        let results = catalog.search("bjork venus");
        assert_eq!(results.tracks.len(), 1);
        let results = catalog.search("bjork human");
        assert_eq!(results.tracks.len(), 1);
        assert_eq!(catalog.search("debut").albums.len(), 1);
        assert_eq!(count(12345, "song", "songs"), "12,345 songs");
    }

    #[test]
    fn artists_filter_and_sort_in_both_directions_or_reshuffle_current_results() {
        let catalog = sample();
        let role = ArtistRole::Track;
        let ascending = catalog.artist_order(role, "", ArtistSort::Name, None);
        let descending = catalog.artist_order(role, "", ArtistSort::ReverseName, None);
        assert!(ascending.iter().eq(descending.iter().rev()));
        assert_eq!(catalog.artists[ascending[0]].name, "Björk");
        assert_eq!(
            catalog.artist_order(role, " Bjork ", ArtistSort::Name, None),
            [ascending[0]]
        );
        let order = catalog.artist_order(role, "e", ArtistSort::Random, None);
        assert_eq!(order.len(), 2);
        let reshuffled = catalog.artist_order(role, "e", ArtistSort::Random, Some(&order));
        assert!(order.iter().eq(reshuffled.iter().rev()));
        assert!(
            catalog
                .artist_order(role, "no such artist", ArtistSort::Random, None)
                .is_empty()
        );
    }

    #[test]
    fn album_and_track_artists_keep_independent_memberships_and_performer_roles() {
        let library = Library::parse(&json!({
            "library": {
                "artists": {"map": {"name": 0}, "1": ["Both roles"], "2": ["Singer"],
                            "3": ["Band credit"], "4": ["A composer"], "5": ["Album collaborator"]},
                "albums": {
                    "map": {"name": 0, "artist_id": 1, "artists_additional": 2},
                    "10": ["Credited album", 1, [[5, "", "artist"]]],
                    "20": ["Guest appearance", 2, []],
                    "30": ["Compilation", 0, []]
                },
                "tracks": {
                    "map": {"title": 0, "artist_id": 1, "album_id": 2, "file": 3, "artists_additional": 4,
                            "artists_additional_map": {"type": 0, "artist_id": 1}},
                    "1": ["Album song", 2, 10, "/128/a/1", []],
                    "2": ["Collaboration", 2, 20, "/128/a/2",
                            [["artist", 1], ["band", 3], ["artist", 1], ["composer", 4]]],
                    "3": ["Other song", 2, 20, "/128/a/3", []],
                    "4": ["Compilation song", 2, 30, "/128/a/4", []]
                }
            }
        })).unwrap();
        let mut library: Library =
            serde_json::from_value(serde_json::to_value(library).unwrap()).unwrap();
        library.prepare();
        let catalog = Catalog::new(library);
        let artist = catalog
            .artist_by_name("Both roles", ArtistRole::Album)
            .unwrap();
        let albums = artist.credits(ArtistRole::Album).unwrap();
        let tracks = artist.credits(ArtistRole::Track).unwrap();
        assert_eq!(albums.albums, [catalog.album_index[&10]]);
        assert_eq!(tracks.albums, [catalog.album_index[&20]]);
        assert_eq!(
            albums
                .tracks
                .iter()
                .map(|&index| catalog.track(index).id)
                .collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(
            tracks
                .tracks
                .iter()
                .map(|&index| catalog.track(index).id)
                .collect::<Vec<_>>(),
            [2]
        );
        assert_eq!(catalog.artist_count(ArtistRole::Album), 4);
        assert_eq!(catalog.artist_count(ArtistRole::Track), 3);
        assert!(
            catalog
                .artist_by_name("Album collaborator", ArtistRole::Album)
                .is_some()
        );
        assert!(
            catalog
                .artist_by_name("Album collaborator", ArtistRole::Track)
                .is_none()
        );
        assert!(
            catalog
                .artist_by_name("Band credit", ArtistRole::Track)
                .is_some()
        );
        assert!(
            catalog
                .artist_by_name("Band credit", ArtistRole::Album)
                .is_none()
        );
        assert!(
            catalog
                .artist_by_name("A composer", ArtistRole::Track)
                .is_none()
        );
        assert!(
            catalog
                .artist_by_name("Various Artists", ArtistRole::Album)
                .is_some()
        );
    }
}
