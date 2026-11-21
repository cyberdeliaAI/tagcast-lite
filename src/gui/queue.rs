//! The session's play queue: order, Play next, shuffle and repeat.

use std::{collections::HashMap, sync::Arc};

use serde::{Deserialize, Serialize};

use tagcast_player::account::Track;

#[derive(Clone)]
pub struct Entry {
    pub uid: u64,
    pub track: Arc<Track>,
    /// Added with Play next. It leaves the queue once it has played.
    pub up_next: bool,
}

#[derive(Default)]
pub struct Queue {
    pub items: Vec<Entry>,
    pub current: Option<usize>,
    /// The order before shuffling, by entry id. `None` while not shuffled.
    original: Option<Vec<u64>>,
    next_uid: u64,
}

const SAVED_LIMIT: usize = 10_000;

#[derive(Clone, Serialize, Deserialize)]
pub struct SavedQueue {
    entries: Vec<(u64, bool)>,
    current: Option<usize>,
    original: Option<Vec<usize>>,
}

pub enum Removed {
    Other,
    /// The playing entry went away. The caller plays the new current entry, if any.
    Current,
}

impl Queue {
    fn entry(&mut self, track: Arc<Track>, up_next: bool) -> Entry {
        self.next_uid += 1;
        Entry {
            uid: self.next_uid,
            track,
            up_next,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn current_entry(&self) -> Option<&Entry> {
        self.items.get(self.current?)
    }

    pub fn shuffled(&self) -> bool {
        self.original.is_some()
    }

    pub fn saved(&self) -> Option<SavedQueue> {
        if self.items.is_empty() || self.items.len() > SAVED_LIMIT {
            return None;
        }
        let index: HashMap<_, _> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, row)| (row.uid, i))
            .collect();
        Some(SavedQueue {
            entries: self
                .items
                .iter()
                .map(|row| (row.track.id, row.up_next))
                .collect(),
            current: self.current,
            original: self.original.as_ref().map(|rows| {
                rows.iter()
                    .filter_map(|id| index.get(id).copied())
                    .collect()
            }),
        })
    }

    pub fn restore(&mut self, saved: SavedQueue, resolve: impl Fn(u64) -> Option<Arc<Track>>) {
        if !self.items.is_empty() || saved.entries.len() > SAVED_LIMIT {
            return;
        }
        let mut remapped = vec![None; saved.entries.len()];
        let mut current = None;
        for (i, (id, up_next)) in saved.entries.into_iter().enumerate() {
            let Some(track) = resolve(id) else {
                continue;
            };
            let entry = self.entry(track, up_next);
            remapped[i] = Some(entry.uid);
            if current.is_none() && saved.current.is_some_and(|old| i >= old) {
                current = Some(self.items.len());
            }
            self.items.push(entry);
        }
        self.current = current.or_else(|| self.items.len().checked_sub(1));
        self.original = saved
            .original
            .filter(|order| order.len() <= SAVED_LIMIT)
            .map(|order| {
                order
                    .into_iter()
                    .filter_map(|i| remapped.get(i).copied().flatten())
                    .collect()
            });
    }

    pub fn next_entry(&self) -> Option<&Entry> {
        self.items.get(self.current? + 1)
    }

    /// Replaces the queue. With no start, a shuffled queue starts on a random track.
    pub fn replace(
        &mut self,
        tracks: Vec<Arc<Track>>,
        start: Option<usize>,
        shuffle: bool,
    ) -> Option<usize> {
        let items: Vec<Entry> = tracks.into_iter().map(|t| self.entry(t, false)).collect();
        self.items = items;
        self.original = None;
        if self.items.is_empty() {
            self.current = None;
            return None;
        }
        let start = match start {
            Some(start) => start.min(self.items.len() - 1),
            None if shuffle => fastrand::usize(..self.items.len()),
            None => 0,
        };
        if shuffle {
            self.original = Some(self.items.iter().map(|e| e.uid).collect());
            let first = self.items.remove(start);
            fastrand::shuffle(&mut self.items);
            self.items.insert(0, first);
            self.current = Some(0);
        } else {
            self.current = Some(start);
        }
        self.current
    }

    /// Adds tracks after the playing track and earlier Play next picks, or at the end.
    pub fn add(&mut self, tracks: Vec<Arc<Track>>, next: bool) {
        let up_next = next && self.current.is_some();
        let entries: Vec<Entry> = tracks.into_iter().map(|t| self.entry(t, up_next)).collect();
        if up_next {
            let start = self.current.unwrap() + 1;
            let offset = start + self.items[start..].iter().take_while(|e| e.up_next).count();
            self.items.splice(offset..offset, entries);
        } else {
            if let Some(original) = &mut self.original {
                original.extend(entries.iter().map(|e| e.uid));
            }
            self.items.extend(entries);
        }
    }

    pub fn remove(&mut self, index: usize) -> Removed {
        if index >= self.items.len() {
            return Removed::Other;
        }
        let entry = self.items.remove(index);
        if let Some(original) = &mut self.original {
            original.retain(|uid| *uid != entry.uid);
        }
        match self.current {
            Some(current) if current == index => {
                if index >= self.items.len() {
                    self.current = None;
                }
                Removed::Current
            }
            Some(current) if index < current => {
                self.current = Some(current - 1);
                Removed::Other
            }
            _ => Removed::Other,
        }
    }

    /// Keeps the playing track and everything before it.
    pub fn clear_upcoming(&mut self) {
        match self.current {
            Some(current) => self.items.truncate(current + 1),
            None => self.items.clear(),
        }
        let kept: std::collections::HashSet<_> = self.items.iter().map(|e| e.uid).collect();
        if let Some(original) = &mut self.original {
            original.retain(|uid| kept.contains(uid));
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.current = None;
        self.original = None;
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffled() {
            return;
        }
        if on {
            self.original = Some(
                self.items
                    .iter()
                    .filter(|e| !e.up_next)
                    .map(|e| e.uid)
                    .collect(),
            );
            let start = self.current.map_or(0, |c| c + 1);
            let pinned = self.items[start.min(self.items.len())..]
                .iter()
                .take_while(|e| e.up_next)
                .count();
            let from = (start + pinned).min(self.items.len());
            fastrand::shuffle(&mut self.items[from..]);
            return;
        }
        let Some(original) = self.original.take() else {
            return;
        };
        let current_uid = self.current_entry().map(|e| e.uid);
        let start = self.current.map_or(0, |c| c + 1);
        let pinned: Vec<Entry> = self.items[start.min(self.items.len())..]
            .iter()
            .take_while(|e| e.up_next)
            .cloned()
            .collect();
        let pinned_ids: std::collections::HashSet<_> = pinned.iter().map(|e| e.uid).collect();
        let mut rest: Vec<Entry> = self
            .items
            .drain(..)
            .filter(|e| !pinned_ids.contains(&e.uid))
            .collect();
        let rank: HashMap<_, _> = original
            .into_iter()
            .enumerate()
            .map(|(i, uid)| (uid, i))
            .collect();
        rest.sort_by_key(|e| rank.get(&e.uid).copied().unwrap_or(usize::MAX));
        self.items = rest;
        self.current = current_uid.and_then(|uid| self.items.iter().position(|e| e.uid == uid));
        let at = self.current.map_or(0, |c| c + 1);
        self.items.splice(at..at, pinned);
    }

    /// Moves to the next entry. `None` means the end of the queue.
    pub fn advance(&mut self, wrap: bool) -> Option<usize> {
        let current = self.current?;
        let mut next = current + 1;
        if self.items.get(current).is_some_and(|e| e.up_next) {
            self.items.remove(current);
            next = current;
        }
        if next >= self.items.len() && wrap {
            next = 0;
        }
        if next < self.items.len() {
            self.current = Some(next);
            Some(next)
        } else {
            self.current = self.items.len().checked_sub(1);
            None
        }
    }

    pub fn previous(&mut self) -> Option<usize> {
        let current = self.current?;
        self.current = Some(current.saturating_sub(1));
        self.current
    }

    pub fn jump(&mut self, index: usize) -> Option<usize> {
        (index < self.items.len()).then(|| {
            self.current = Some(index);
            index
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracks(ids: &[u64]) -> Vec<Arc<Track>> {
        ids.iter()
            .map(|&id| {
                Arc::new(Track {
                    id,
                    title: format!("T{id}"),
                    artist: String::new(),
                    album: String::new(),
                    album_id: 0,
                    disc: 0,
                    number: id,
                    seconds: 60.0,
                    path: format!("/128/x/{id}"),
                    mime: String::new(),
                    artwork: 0,
                    year: 0,
                    album_artist: String::new(),
                    search: String::new(),
                })
            })
            .collect()
    }

    fn ids(queue: &Queue) -> Vec<u64> {
        queue.items.iter().map(|e| e.track.id).collect()
    }

    #[test]
    fn play_next_goes_after_earlier_picks_and_leaves_after_playing() {
        let mut queue = Queue::default();
        queue.replace(tracks(&[1, 2, 3]), Some(0), false);
        queue.add(tracks(&[8]), true);
        queue.add(tracks(&[9]), true);
        queue.add(tracks(&[7]), false);
        assert_eq!(ids(&queue), [1, 8, 9, 2, 3, 7]);
        assert_eq!(queue.advance(false), Some(1));
        assert_eq!(queue.advance(false), Some(1));
        assert_eq!(ids(&queue), [1, 9, 2, 3, 7]);
        assert_eq!(queue.current_entry().unwrap().track.id, 9);
        assert_eq!(queue.advance(false), Some(1));
        assert_eq!(ids(&queue), [1, 2, 3, 7]);
    }

    #[test]
    fn shuffle_keeps_the_current_track_and_restores_the_order() {
        let mut queue = Queue::default();
        queue.replace(tracks(&(1..=40).collect::<Vec<_>>()), Some(4), false);
        queue.add(tracks(&[99]), true);
        queue.set_shuffle(true);
        assert_eq!(queue.current_entry().unwrap().track.id, 5);
        assert_eq!(queue.items[5].track.id, 99, "Play next stays next");
        queue.set_shuffle(false);
        let mut expected: Vec<u64> = (1..=5).collect();
        expected.push(99);
        expected.extend(6..=40);
        assert_eq!(ids(&queue), expected);
        assert_eq!(queue.current, Some(4));
    }

    #[test]
    fn shuffled_start_begins_with_the_chosen_track() {
        let mut queue = Queue::default();
        queue.replace(tracks(&(1..=20).collect::<Vec<_>>()), Some(7), true);
        assert_eq!(queue.current, Some(0));
        assert_eq!(queue.items[0].track.id, 8);
        assert!(queue.shuffled());
        let mut sorted = ids(&queue);
        sorted.sort();
        assert_eq!(sorted, (1..=20).collect::<Vec<_>>());
    }

    #[test]
    fn restore_removes_missing_ids_and_keeps_occurrences_and_play_next() {
        let mut source = Queue::default();
        source.replace(tracks(&[1, 2, 1, 3]), Some(1), false);
        source.add(tracks(&[8]), true);
        let saved = source.saved().unwrap();
        let mut target = Queue::default();
        target.restore(saved, |id| (id != 2).then(|| tracks(&[id]).remove(0)));
        assert_eq!(ids(&target), [1, 8, 1, 3]);
        assert!(target.current_entry().unwrap().up_next);
        assert_eq!(target.advance(false), Some(1));
        assert_eq!(ids(&target), [1, 1, 3]);
    }

    #[test]
    fn removing_and_wrapping_track_the_current_entry() {
        let mut queue = Queue::default();
        queue.replace(tracks(&[1, 2, 3]), Some(1), false);
        assert!(matches!(queue.remove(0), Removed::Other));
        assert_eq!(queue.current, Some(0));
        assert!(matches!(queue.remove(0), Removed::Current));
        assert_eq!(queue.current_entry().unwrap().track.id, 3);
        assert_eq!(queue.advance(false), None);
        assert_eq!(queue.current, Some(0));
        assert_eq!(queue.advance(true), Some(0));
        queue.clear_upcoming();
        assert_eq!(ids(&queue), [3]);
    }
}
