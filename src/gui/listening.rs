//! Local listening state, bound to the verified iBroadcast account.
use std::{fs::File, io::BufReader, path::PathBuf, sync::mpsc, thread};

use serde::{Deserialize, Serialize};
use tagcast_player::account::{data_dir, write_private};

use super::queue::SavedQueue;

pub const HISTORY_LIMIT: usize = 250;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Listening {
    pub queue: Option<SavedQueue>,
    pub recent: Vec<u64>,
    pub albums: Vec<u64>,
}

impl Listening {
    pub fn load(owner: &str) -> Self {
        let Some(file) = path(owner).and_then(|path| File::open(path).ok()) else {
            return Self::default();
        };
        if !file
            .metadata()
            .is_ok_and(|metadata| metadata.len() <= 1_048_576)
        {
            return Self::default();
        }
        let mut data: Self = serde_json::from_reader(BufReader::new(file)).unwrap_or_default();
        data.recent.truncate(HISTORY_LIMIT);
        data.albums.truncate(10_000);
        data
    }

    pub fn record(&mut self, track: u64) -> bool {
        if self.recent.first() == Some(&track) {
            return false;
        }
        self.recent.retain(|id| *id != track);
        self.recent.insert(0, track);
        self.recent.truncate(HISTORY_LIMIT);
        true
    }

    pub fn toggle_album(&mut self, album: u64) {
        if let Some(index) = self.albums.iter().position(|id| *id == album) {
            self.albums.remove(index);
        } else if self.albums.len() < 10_000 {
            self.albums.push(album);
        }
    }
}

fn path(owner: &str) -> Option<PathBuf> {
    let id = owner.parse::<u64>().ok().filter(|id| *id != 0)?;
    Some(data_dir()?.join(format!("listening-{id}.json")))
}

pub struct Writer {
    send: Option<mpsc::Sender<(PathBuf, Listening)>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Writer {
    pub fn new() -> Self {
        let (send, receive) = mpsc::channel::<(PathBuf, Listening)>();
        let worker = thread::Builder::new()
            .name("listening-storage".into())
            .spawn(move || {
                while let Ok((path, data)) = receive.recv() {
                    let _ = write_private(&path, |file| {
                        serde_json::to_writer(file, &data).map_err(std::io::Error::other)
                    });
                }
            })
            .ok();
        Self {
            send: Some(send),
            worker,
        }
    }

    pub fn save(&self, owner: &str, data: Listening) {
        if let (Some(path), Some(send)) = (path(owner), &self.send) {
            let _ = send.send((path, data));
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.send.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
