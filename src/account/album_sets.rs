//! Read-only projections of iBroadcast disc rows and native combined containers.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Track, additional_artists, clean, entry, number, truth};

#[derive(Deserialize, Serialize)]
pub struct AlbumSet {
    pub name: String,
    pub artist: String,
    pub year: u32,
    pub added: u64,
    pub rating: u8,
    pub discs: Vec<AlbumDisc>,
}

#[derive(Deserialize, Serialize)]
pub struct AlbumDisc {
    pub number: u64,
    pub tracks: Vec<u64>,
    pub encoded: bool,
}

pub(super) struct TrackLayout<'a> {
    pub day: &'a str,
    pub added: u64,
    pub numeric: bool,
}

struct AlbumRow<'a> {
    name: &'a str,
    artist: &'a str,
    extra_artists: Vec<u64>,
    year: u32,
    disc: u64,
    rating: u8,
    members: Vec<usize>,
}

pub(super) fn explicit_position(raw: &Value) -> Option<(u64, u64)> {
    let (disc, track) = raw.as_str()?.split_once('-')?;
    let valid = |part: &str| {
        !part.is_empty() && part.len() <= 4 && part.bytes().all(|byte| byte.is_ascii_digit())
    };
    if !valid(disc) || disc.starts_with('0') || !valid(track) {
        return None;
    }
    let (disc, track) = (disc.parse().ok()?, track.parse().ok()?);
    (track > 0).then_some((disc, track))
}

fn additional_album_artist_ids(row: &Value, albums: &Value, primary: u64) -> Vec<u64> {
    let mut seen = HashSet::from([0, primary]);
    additional_artists(row, albums)
        .map(|(id, _)| id)
        .filter(|id| seen.insert(*id))
        .collect()
}

/// A single 101, a sparse block or duplicate positions are native numbers.
fn encoded_discs(
    members: &[usize],
    tracks: &[Arc<Track>],
    layout: &[TrackLayout<'_>],
) -> Option<BTreeMap<u64, Vec<usize>>> {
    let mut discs = BTreeMap::<u64, Vec<usize>>::new();
    for &index in members {
        let position = tracks[index].number;
        if !layout[index].numeric
            || !(101..=9999).contains(&position)
            || position.is_multiple_of(100)
        {
            return None;
        }
        discs.entry(position / 100).or_default().push(index);
    }
    if discs.len() < 2 {
        return None;
    }
    for (expected, (&disc, rows)) in (1..).zip(&mut discs) {
        rows.sort_unstable_by_key(|&index| tracks[index].number);
        if disc != expected
            || rows
                .iter()
                .enumerate()
                .any(|(i, &index)| tracks[index].number % 100 != i as u64 + 1)
        {
            return None;
        }
    }
    Some(discs)
}

fn make_set(
    album: &AlbumRow<'_>,
    sections: impl IntoIterator<Item = (u64, Vec<usize>)>,
    encoded: bool,
    rating: u8,
    tracks: &[Arc<Track>],
    layout: &[TrackLayout<'_>],
) -> AlbumSet {
    let mut added = 0;
    let discs = sections
        .into_iter()
        .map(|(number, mut members)| {
            members.sort_unstable_by_key(|&index| {
                let track = &tracks[index];
                (track.number == 0, track.number, track.id)
            });
            let ids = members
                .into_iter()
                .map(|index| {
                    added = added.max(layout[index].added);
                    tracks[index].id
                })
                .collect();
            AlbumDisc {
                number,
                tracks: ids,
                encoded,
            }
        })
        .collect();
    AlbumSet {
        name: clean(album.name),
        artist: clean(album.artist),
        year: album.year,
        added,
        rating,
        discs,
    }
}

fn native_sections(members: &[usize], tracks: &[Arc<Track>]) -> Vec<(u64, Vec<usize>)> {
    let mut slots = HashMap::new();
    let mut sections = Vec::<(u64, Vec<usize>)>::new();
    for &index in members {
        let track = &tracks[index];
        let slot = *slots
            .entry((track.album_id, track.disc))
            .or_insert_with(|| {
                sections.push((track.disc, Vec::new()));
                sections.len() - 1
            });
        sections[slot].1.push(index);
    }
    // Unknown disc numbers retain their order in the returned container.
    sections.sort_by_key(|(disc, _)| (*disc == 0, *disc));
    sections
}

pub(super) fn build(
    albums: &Value,
    artists: &Value,
    tracks: &[Arc<Track>],
    layout: &[TrackLayout<'_>],
    combined: bool,
) -> BTreeMap<u64, AlbumSet> {
    let index: HashMap<_, _> = tracks
        .iter()
        .enumerate()
        .map(|(i, track)| (track.id, i))
        .collect();
    let mut by_source = HashMap::<u64, Vec<usize>>::new();
    for (i, track) in tracks.iter().enumerate() {
        by_source.entry(track.album_id).or_default().push(i);
    }
    let mut eligible = BTreeMap::new();
    let mut foreign_parents = HashMap::<u64, Option<u64>>::new();
    for (id, row) in albums.as_object().into_iter().flatten() {
        let Ok(id) = id.parse::<u64>() else { continue };
        if id == 0 || truth(entry(row, albums, "trashed")) {
            continue;
        }
        let members = match entry(row, albums, "tracks").as_array() {
            Some(stored) => {
                let mut seen = HashSet::new();
                stored
                    .iter()
                    .filter_map(|track| index.get(&number(track)).copied())
                    .filter(|i| seen.insert(*i))
                    .collect()
            }
            None => by_source.get(&id).cloned().unwrap_or_default(),
        };
        if members.is_empty() {
            continue;
        }
        for &i in &members {
            let source = tracks[i].album_id;
            if source != id {
                let parent = foreign_parents.entry(source).or_insert(Some(id));
                if *parent != Some(id) {
                    *parent = None;
                }
            }
        }
        let primary = number(entry(row, albums, "artist_id"));
        let artist = if primary == 0 {
            "Various Artists"
        } else {
            entry(&artists[primary.to_string()], artists, "name")
                .as_str()
                .unwrap_or("Unknown artist")
        };
        let year = number(entry(row, albums, "year"));
        let rating = number(entry(row, albums, "rating"));
        eligible.insert(
            id,
            AlbumRow {
                name: entry(row, albums, "name")
                    .as_str()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or("Unknown album"),
                artist,
                extra_artists: additional_album_artist_ids(row, albums, primary),
                year: if year <= 9999 { year as u32 } else { 0 },
                disc: number(entry(row, albums, "disc")),
                rating: if rating <= 5 { rating as u8 } else { 0 },
                members,
            },
        );
    }

    let hidden: HashSet<_> = foreign_parents
        .iter()
        .filter_map(|(&source, &parent)| {
            let (source_row, parent) = (eligible.get(&source)?, eligible.get(&parent?)?);
            if source_row.members.len() >= parent.members.len() {
                return None;
            }
            let members: HashSet<_> = parent.members.iter().copied().collect();
            source_row
                .members
                .iter()
                .all(|member| members.contains(member))
                .then_some(source)
        })
        .collect();
    let mut claims = vec![0u8; tracks.len()];
    for (&id, album) in &eligible {
        if !hidden.contains(&id) {
            for &member in &album.members {
                claims[member] = claims[member].saturating_add(1);
            }
        }
    }

    let mut sets = BTreeMap::new();
    let mut native_view = combined || !foreign_parents.is_empty();
    for (&id, album) in &eligible {
        if hidden.contains(&id) {
            continue;
        }
        let encoded = encoded_discs(&album.members, tracks, layout);
        native_view |= encoded.is_some();
        let owns_all = album
            .members
            .iter()
            .filter(|&&i| tracks[i].album_id == id)
            .count()
            == by_source.get(&id).map_or(0, Vec::len);
        if !owns_all || album.members.iter().any(|&i| claims[i] != 1) {
            continue;
        }
        if combined && let Some(encoded) = encoded {
            sets.insert(
                id,
                make_set(album, encoded, true, album.rating, tracks, layout),
            );
            continue;
        }
        let foreign = album.members.iter().any(|&i| tracks[i].album_id != id);
        let disc = tracks[album.members[0]].disc;
        if foreign || album.members.iter().any(|&i| tracks[i].disc != disc) {
            let sections = native_sections(&album.members, tracks);
            sets.insert(
                id,
                make_set(album, sections, false, album.rating, tracks, layout),
            );
        }
    }
    if native_view {
        return sets;
    }

    let mut candidates = HashMap::<_, Vec<u64>>::new();
    for (&id, album) in &eligible {
        if sets.contains_key(&id) || album.members.len() != by_source.get(&id).map_or(0, Vec::len) {
            continue;
        }
        let mut days: Vec<_> = album.members.iter().map(|&i| layout[i].day).collect();
        days.sort_unstable();
        days.dedup();
        let key = (
            album.name.trim().to_lowercase(),
            album.artist,
            album.year,
            days,
            &album.extra_artists,
        );
        candidates.entry(key).or_default().push(id);
    }
    for mut ids in candidates.into_values().filter(|ids| ids.len() > 1) {
        ids.sort_unstable_by_key(|id| eligible[id].disc);
        if ids
            .windows(2)
            .any(|pair| eligible[&pair[0]].disc == eligible[&pair[1]].disc)
        {
            continue;
        }
        let first = ids[0];
        let rating = ids.iter().map(|id| eligible[id].rating).min().unwrap_or(0);
        let sections = ids.iter().map(|id| {
            let album = &eligible[id];
            (album.disc, album.members.clone())
        });
        sets.insert(
            first,
            make_set(&eligible[&first], sections, false, rating, tracks, layout),
        );
    }
    sets
}
