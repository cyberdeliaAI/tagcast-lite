//! Send the current listening state to the OS and turn media keys into normal actions.
use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use eframe::egui;
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};
use tagcast_player::{
    APP_NAME, CancellationToken, Url,
    account::{Http, check_status},
    audio::{Phase, Snapshot},
};

use super::queue::Entry;

struct Cover {
    url: String,
    file: Option<tempfile::NamedTempFile>,
}

pub struct Media {
    controls: MediaControls,
    pub events: mpsc::Receiver<MediaControlEvent>,
    key: Option<u64>,
    state: u8,
    clock: Instant,
    cover_url: Option<String>,
    cover: Option<Cover>,
    cover_stop: CancellationToken,
    covers: mpsc::Receiver<Cover>,
    worker: mpsc::Sender<Cover>,
    http: Option<Http>,
    ctx: egui::Context,
}

impl Media {
    pub fn new(_frame: &eframe::Frame, ctx: egui::Context) -> Option<Self> {
        #[cfg(target_os = "windows")]
        let hwnd = {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let RawWindowHandle::Win32(handle) = _frame.window_handle().ok()?.as_raw() else {
                return None;
            };
            Some(handle.hwnd.get() as *mut std::ffi::c_void)
        };
        #[cfg(not(target_os = "windows"))]
        let hwnd = None;
        let mut controls = MediaControls::new(PlatformConfig {
            dbus_name: "tagcast_player",
            display_name: APP_NAME,
            hwnd,
        })
        .ok()?;
        let (send, events) = mpsc::channel();
        let repaint = ctx.clone();
        controls
            .attach(move |event| {
                let _ = send.send(event);
                repaint.request_repaint();
            })
            .ok()?;
        let (worker, covers) = mpsc::channel();
        Some(Self {
            controls,
            events,
            key: None,
            state: 0,
            clock: Instant::now(),
            cover_url: None,
            cover: None,
            cover_stop: CancellationToken::new(),
            worker,
            covers,
            http: None,
            ctx,
        })
    }

    #[cfg(target_os = "linux")]
    pub fn volume(&mut self, gain: f64) {
        let _ = self.controls.set_volume(gain);
    }

    pub fn update(&mut self, entry: Option<&Entry>, state: &Snapshot, cover: Option<String>) {
        let active = entry.filter(|_| state.phase != Phase::Idle);
        let cover = cover.filter(|_| active.is_some());
        let key = active.map(|entry| entry.uid);
        if cover != self.cover_url {
            self.cover_stop.cancel();
            self.cover_stop = CancellationToken::new();
            self.cover_url = cover;
            self.cover = None;
            if let Some(url) = self.cover_url.clone() {
                self.http = self.http.take().or_else(|| Http::new().ok());
                if let Some(http) = self.http.clone() {
                    let stop = self.cover_stop.clone();
                    let (send, ctx) = (self.worker.clone(), self.ctx.clone());
                    thread::spawn(move || {
                        let file = fetch(&http, &stop, &url);
                        if !stop.is_cancelled() {
                            let _ = send.send(Cover { url, file });
                            ctx.request_repaint();
                        }
                    });
                }
            }
        }
        let mut added = false;
        while let Ok(cover) = self.covers.try_recv() {
            if self.cover_url.as_deref() == Some(&cover.url) {
                self.cover = Some(cover);
                added = true;
            }
        }
        let changed = key != self.key;
        if changed || added {
            let file = self
                .cover
                .as_ref()
                .and_then(|cover| cover.file.as_ref())
                .and_then(|file| Url::from_file_path(file.path()).ok());
            let metadata = active.map_or(MediaMetadata::default(), |entry| MediaMetadata {
                title: Some(&entry.track.title),
                artist: Some(&entry.track.artist),
                album: Some(&entry.track.album),
                cover_url: file.as_ref().map(Url::as_str),
                duration: state
                    .duration
                    .and_then(|d| Duration::try_from_secs_f64(d).ok()),
            });
            let _ = self.controls.set_metadata(metadata);
            self.key = key;
        }
        let progress = Duration::try_from_secs_f64(state.position)
            .ok()
            .map(MediaPosition);
        let (mode, playback) = if active.is_none() {
            (0, MediaPlayback::Stopped)
        } else if state.paused || matches!(state.phase, Phase::Ended | Phase::Failed(_)) {
            (1, MediaPlayback::Paused { progress })
        } else {
            (2, MediaPlayback::Playing { progress })
        };
        if changed
            || mode != self.state
            || (mode != 0 && self.clock.elapsed() >= Duration::from_secs(1))
        {
            let _ = self.controls.set_playback(playback);
            self.state = mode;
            self.clock = Instant::now();
        }
    }
}

fn fetch(http: &Http, stop: &CancellationToken, url: &str) -> Option<tempfile::NamedTempFile> {
    let url = Url::parse(url).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let response = check_status(http.send(stop, http.client.get(url)).ok()?).ok()?;
    let bytes = http.body(stop, response, 1_048_576, 5).ok()?;
    // Write only decoded and re-encoded artwork. The OS never opens a remote URL.
    let image = super::artwork::decode(&bytes, 300)?;
    let mut file = tempfile::Builder::new().suffix(".png").tempfile().ok()?;
    let bytes: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|pixel| pixel.to_array())
        .collect();
    use image::ImageEncoder;
    image::codecs::png::PngEncoder::new(file.as_file_mut())
        .write_image(
            &bytes,
            image.size[0] as u32,
            image.size[1] as u32,
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
    Some(file)
}

impl Drop for Media {
    fn drop(&mut self) {
        self.cover_stop.cancel();
    }
}
