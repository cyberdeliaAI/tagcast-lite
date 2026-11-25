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

const TEXTURE_BUDGET: usize = 64 * 1024 * 1024;
const MAX_TEXTURES: usize = 180;
const MAX_BYTES: usize = 6 * 1024 * 1024;
/// A queued fetch is dropped when nothing asked for its cover for this long.
const STALE: Duration = Duration::from_millis(1500);

/// A failed download is tried again after this long.
const RETRY: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ArtSize {
    Small = 150,
    Grid = 300,
    Detail = 1000,
}

impl ArtSize {
    const ALL: [Self; 3] = [Self::Small, Self::Grid, Self::Detail];

    pub fn album_detail(pixels_per_point: f32) -> Self {
        // A 300 px cover already looks sharp at the normal 168 pt Retina size.
        if 168.0 * pixels_per_point > 360.0 {
            Self::Detail
        } else {
            Self::Grid
        }
    }
}

type Key = (u64, ArtSize);

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
    wanted: Arc<Mutex<HashMap<Key, Instant>>>,
    slots: HashMap<Key, Slot>,
    send: Sender<(Key, Loaded)>,
    receive: Receiver<(Key, Loaded)>,
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
        let mut uploaded = false;
        while let Ok((artwork, loaded)) = self.receive.try_recv() {
            match loaded {
                Loaded::Image(image) => {
                    uploaded = true;
                    // Keep one resident copy per ID, including when concurrent sizes arrive out of order.
                    if ArtSize::ALL.iter().any(|&size| {
                        size > artwork.1
                            && matches!(self.slots.get(&(artwork.0, size)), Some(Slot::Ready(..)))
                    }) {
                        self.slots.remove(&artwork);
                        continue;
                    }
                    for size in ArtSize::ALL {
                        let key = (artwork.0, size);
                        if size < artwork.1 && matches!(self.slots.get(&key), Some(Slot::Ready(..)))
                        {
                            self.slots.remove(&key);
                        }
                    }
                    let texture = self.ctx.load_texture(
                        format!("artwork-{}-{}", artwork.0, artwork.1 as u32),
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
        if uploaded {
            self.trim();
        }
    }

    fn trim(&mut self) {
        let mut count = 0;
        let mut bytes = 0;
        for slot in self.slots.values() {
            if let Slot::Ready(texture, _) = slot {
                count += 1;
                bytes += texture.byte_size();
            }
        }
        if count <= MAX_TEXTURES && bytes <= TEXTURE_BUDGET {
            return;
        }
        let mut ages: Vec<_> = self
            .slots
            .iter()
            .filter_map(|(key, slot)| match slot {
                Slot::Ready(texture, used) => Some((*used, *key, texture.byte_size())),
                _ => None,
            })
            .collect();
        ages.sort_unstable();
        for (_, key, size) in ages {
            if count <= MAX_TEXTURES && bytes <= TEXTURE_BUDGET {
                break;
            }
            self.slots.remove(&key);
            count -= 1;
            bytes -= size;
        }
    }

    /// Forget everything, for example after signing out.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.wanted.lock().unwrap().clear();
        // Old workers can only deliver to the discarded receiver.
        self.wanted = Arc::default();
        (self.send, self.receive) = channel();
    }

    pub fn unavailable(&self, artwork: u64, size: ArtSize) -> bool {
        matches!(
            self.slots.get(&(artwork, size)),
            Some(Slot::Missing | Slot::Failed(_))
        )
    }

    /// Reuse a larger cover immediately. During an upgrade keep showing the thumbnail.
    pub fn get(
        &mut self,
        id: u64,
        size: ArtSize,
        library: &tagcast_player::account::Library,
    ) -> Option<(egui::TextureId, [usize; 2])> {
        if id == 0 {
            return None;
        }
        let mut preview = None;
        let mut pending = None;
        for candidate in ArtSize::ALL.into_iter().rev() {
            match self.slots.get_mut(&(id, candidate)) {
                Some(Slot::Ready(texture, used)) => {
                    *used = self.frame;
                    let image = (texture.id(), texture.size());
                    if candidate >= size {
                        return Some(image);
                    }
                    preview = Some(image);
                }
                Some(Slot::Loading) if candidate >= size => pending = Some(candidate),
                _ => {}
            }
        }
        if let Some(candidate) = pending {
            self.wanted
                .lock()
                .unwrap()
                .insert((id, candidate), Instant::now());
            return preview;
        }
        let artwork = (id, size);
        match self.slots.get(&artwork) {
            Some(Slot::Missing) => return preview,
            Some(Slot::Failed(at)) if at.elapsed() < RETRY => return preview,
            _ => {}
        }
        let (Some(url), Some(runtime), Some(client)) = (
            library.artwork_url(id, size as u32),
            &self.runtime,
            &self.client,
        ) else {
            self.slots.insert(artwork, Slot::Missing);
            return preview;
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
                        fetch(&client, &url, size).await
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
        preview
    }
}

async fn fetch(client: &Client, url: &str, size: ArtSize) -> Loaded {
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
    match decode(&body, size as u32) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tagcast_player::account::Library;

    fn deliver(art: &mut Artwork, id: u64, size: ArtSize, edge: usize) {
        let img = ColorImage::filled([edge, edge], egui::Color32::WHITE);
        art.send.send(((id, size), Loaded::Image(img))).unwrap();
        art.begin_frame();
    }

    #[test]
    fn upgrades_keep_the_preview_and_reuse_the_larger_image() {
        let mut art = Artwork::new(egui::Context::default());
        let library = Library::default();
        deliver(&mut art, 1, ArtSize::Small, 150);
        art.slots.insert((1, ArtSize::Grid), Slot::Loading);
        assert_eq!(art.get(1, ArtSize::Grid, &library).unwrap().1, [150; 2]);
        deliver(&mut art, 1, ArtSize::Grid, 300);
        deliver(&mut art, 1, ArtSize::Small, 150);
        assert_eq!(art.get(1, ArtSize::Small, &library).unwrap().1, [300; 2]);
        assert_eq!(art.slots.len(), 1);
    }

    #[test]
    fn clear_discards_old_deliveries_and_keeps_new_requests_independent() {
        let mut art = Artwork::new(egui::Context::default());
        let old = art.send.clone();
        art.clear();
        assert!(old.send(((1, ArtSize::Small), Loaded::Missing)).is_err());
        deliver(&mut art, 1, ArtSize::Grid, 300);
        assert!(!art.unavailable(1, ArtSize::Grid));
    }

    #[test]
    fn detail_downloads_follow_the_display_resolution() {
        assert_eq!(ArtSize::album_detail(1.0), ArtSize::Grid);
        assert_eq!(ArtSize::album_detail(2.0), ArtSize::Grid);
        assert_eq!(ArtSize::album_detail(3.0), ArtSize::Detail);
    }
}
