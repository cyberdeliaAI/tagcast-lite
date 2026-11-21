//! Cover art: fetched in the background, decoded off the UI thread and kept as a bounded set of textures.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, Sender, channel},
    },
    time::{Duration, Instant},
};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use reqwest::{Client, redirect::Policy};
use tokio::{runtime::Runtime, sync::Semaphore};

/// About 64 MB of decoded 300 px covers.
const MAX_TEXTURES: usize = 180;
const MAX_BYTES: usize = 6 * 1024 * 1024;
/// A queued fetch is dropped when nothing asked for its cover for this long.
const STALE: Duration = Duration::from_millis(1500);

/// A failed download is tried again after this long.
const RETRY: Duration = Duration::from_secs(30);

enum Slot {
    Loading,
    Ready(TextureHandle, u64),
    Missing,
    Failed(Instant),
}

enum Loaded {
    Image(ColorImage),
    Missing,
    Failed,
    Skipped,
}

pub struct Artwork {
    ctx: egui::Context,
    runtime: Option<Runtime>,
    client: Option<Client>,
    permits: Arc<Semaphore>,
    wanted: Arc<Mutex<HashMap<u64, Instant>>>,
    slots: HashMap<u64, Slot>,
    send: Sender<(u64, Loaded)>,
    receive: Receiver<(u64, Loaded)>,
    frame: u64,
}

impl Artwork {
    pub fn new(ctx: egui::Context) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("artwork")
            .enable_all()
            .build()
            .ok();
        let client = Client::builder()
            .redirect(Policy::limited(3))
            .user_agent(concat!("Tagcast-Player/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(6))
            .timeout(Duration::from_secs(20))
            .build()
            .ok();
        let (send, receive) = channel();
        Self {
            ctx,
            runtime,
            client,
            permits: Arc::new(Semaphore::new(6)),
            wanted: Arc::default(),
            slots: HashMap::new(),
            send,
            receive,
            frame: 0,
        }
    }

    /// Call once per frame before drawing.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        while let Ok((artwork, loaded)) = self.receive.try_recv() {
            match loaded {
                Loaded::Image(image) => {
                    let texture = self.ctx.load_texture(
                        format!("artwork-{artwork}"),
                        image,
                        TextureOptions::LINEAR,
                    );
                    self.slots.insert(artwork, Slot::Ready(texture, self.frame));
                }
                Loaded::Missing => {
                    self.slots.insert(artwork, Slot::Missing);
                }
                Loaded::Failed => {
                    self.slots.insert(artwork, Slot::Failed(Instant::now()));
                }
                Loaded::Skipped => {
                    self.slots.remove(&artwork);
                }
            }
        }
        let ready = self
            .slots
            .values()
            .filter(|s| matches!(s, Slot::Ready(..)))
            .count();
        if ready > MAX_TEXTURES {
            let mut ages: Vec<(u64, u64)> = self
                .slots
                .iter()
                .filter_map(|(id, slot)| match slot {
                    Slot::Ready(_, used) if *used + 2 < self.frame => Some((*used, *id)),
                    _ => None,
                })
                .collect();
            ages.sort_unstable();
            for (_, id) in ages.into_iter().take(ready - MAX_TEXTURES) {
                self.slots.remove(&id);
            }
        }
    }

    /// Forget everything, for example after signing out.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.wanted.lock().unwrap().clear();
    }

    pub fn unavailable(&self, artwork: u64) -> bool {
        matches!(
            self.slots.get(&artwork),
            Some(Slot::Missing | Slot::Failed(_))
        )
    }

    pub fn size(&self, artwork: u64) -> Option<[usize; 2]> {
        match self.slots.get(&artwork) {
            Some(Slot::Ready(texture, _)) => Some(texture.size()),
            _ => None,
        }
    }

    /// The cover texture, or `None` while it loads or when there is none.
    pub fn get(&mut self, artwork: u64, url: Option<String>) -> Option<egui::TextureId> {
        if artwork == 0 {
            return None;
        }
        match self.slots.get_mut(&artwork) {
            Some(Slot::Ready(texture, used)) => {
                *used = self.frame;
                return Some(texture.id());
            }
            Some(Slot::Missing) => return None,
            Some(Slot::Failed(at)) if at.elapsed() < RETRY => return None,
            Some(Slot::Failed(_)) => {}
            Some(Slot::Loading) => {
                self.wanted.lock().unwrap().insert(artwork, Instant::now());
                return None;
            }
            None => {}
        }
        let (Some(url), Some(runtime), Some(client)) = (url, &self.runtime, &self.client) else {
            self.slots.insert(artwork, Slot::Missing);
            return None;
        };
        self.slots.insert(artwork, Slot::Loading);
        self.wanted.lock().unwrap().insert(artwork, Instant::now());
        let (client, permits, wanted, send, ctx) = (
            client.clone(),
            self.permits.clone(),
            self.wanted.clone(),
            self.send.clone(),
            self.ctx.clone(),
        );
        runtime.spawn(async move {
            let loaded = match permits.acquire().await {
                Ok(_permit) => {
                    let fresh = wanted
                        .lock()
                        .unwrap()
                        .get(&artwork)
                        .is_some_and(|t| t.elapsed() < STALE);
                    if fresh {
                        fetch(&client, &url).await
                    } else {
                        Loaded::Skipped
                    }
                }
                Err(_) => Loaded::Skipped,
            };
            wanted.lock().unwrap().remove(&artwork);
            let _ = send.send((artwork, loaded));
            ctx.request_repaint();
        });
        None
    }
}

async fn fetch(client: &Client, url: &str) -> Loaded {
    let Ok(mut response) = client.get(url).send().await else {
        return Loaded::Failed;
    };
    if response.status().is_server_error() || response.status().as_u16() == 429 {
        return Loaded::Failed;
    }
    if !response.status().is_success() {
        return Loaded::Missing;
    }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > MAX_BYTES {
                    return Loaded::Missing;
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => return Loaded::Failed,
        }
    }
    match decode(&body, 320) {
        Some(image) => Loaded::Image(image),
        None => Loaded::Missing,
    }
}

pub fn decode(bytes: &[u8], max: u32) -> Option<ColorImage> {
    let image = image::load_from_memory(bytes).ok()?;
    let image = if image.width() > max || image.height() > max {
        image.thumbnail(max, max)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}
