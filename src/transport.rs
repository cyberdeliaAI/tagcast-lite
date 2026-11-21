mod hls;

use std::{
    collections::VecDeque,
    io::{self, Read, Seek, SeekFrom},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

use reqwest::{Response, header};
use symphonia::core::io::MediaSource;

use crate::{
    CancellationToken, Failure, Result, Url,
    account::{Account, Http, Track, check_status, http_error},
};

const STREAM: &str = "https://streaming.ibroadcast.com";
const CACHE_BYTES: usize = 24 * 1024 * 1024;
const RETAIN_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    K128,
    Original,
    K96,
    K192,
    K256,
    K320,
}

impl Quality {
    pub fn value(self) -> &'static str {
        match self {
            Self::K128 => "128",
            Self::Original => "orig",
            Self::K96 => "96",
            Self::K192 => "192",
            Self::K256 => "256",
            Self::K320 => "320",
        }
    }

    pub fn hls(self) -> bool {
        !matches!(self, Self::K128 | Self::Original)
    }
}

pub fn valid_track_path(path: &str) -> bool {
    let Some((quality, rest)) = path.strip_prefix('/').and_then(|p| p.split_once('/')) else {
        return false;
    };
    ["128", "orig", "96", "192", "256", "320"].contains(&quality)
        && path.len() <= 2048
        && !rest.is_empty()
        && rest.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-.~".contains(&c))
        })
}

#[derive(Clone)]
pub struct MediaRequest {
    pub http: Http,
    location: Location,
    pub quality: Quality,
}

#[derive(Clone)]
enum Location {
    Native {
        account: Arc<Account>,
        track: Arc<Track>,
        owner: String,
        expires: String,
    },
    Fixture(Arc<Fixture>),
}

struct Fixture {
    direct: Url,
    hls: Option<Url>,
    fallback: Option<Url>,
    extension: &'static str,
}

impl MediaRequest {
    pub fn native(
        account: Arc<Account>,
        track: Arc<Track>,
        owner: &str,
        expires: &str,
        quality: Quality,
        premium: bool,
    ) -> Self {
        Self {
            http: account.http.clone(),
            location: Location::Native {
                account,
                track,
                owner: owner.into(),
                expires: expires.into(),
            },
            quality: if premium { quality } else { Quality::K128 },
        }
    }

    #[doc(hidden)]
    pub fn fixture(
        http: Http,
        direct: Url,
        hls: Option<Url>,
        fallback: Option<Url>,
        extension: &'static str,
        quality: Quality,
    ) -> Result<Self> {
        for url in std::iter::once(&direct)
            .chain(hls.as_ref())
            .chain(fallback.as_ref())
        {
            if url.scheme() != "http"
                || url.host_str() != Some("127.0.0.1")
                || url.origin() != direct.origin()
            {
                return Err(Failure::Security);
            }
        }
        Ok(Self {
            http,
            location: Location::Fixture(Arc::new(Fixture {
                direct,
                hls,
                fallback,
                extension,
            })),
            quality,
        })
    }

    fn get(
        &self,
        stop: &CancellationToken,
        quality: Quality,
        hls: bool,
        range: Option<u64>,
    ) -> Result<Response> {
        let request = |url: Url| media_get(&self.http, stop, url, range);
        match &self.location {
            Location::Native {
                account,
                track,
                owner,
                expires,
            } => {
                if !valid_track_path(&track.path) {
                    return Err(Failure::Unsupported);
                }
                let (_, file) = track.path[1..].split_once('/').unwrap();
                let endpoint = if hls {
                    format!("hls_{}", quality.value())
                } else {
                    quality.value().into()
                };
                account.authorized(stop, |token| {
                    let mut url = Url::parse(&format!("{STREAM}/{endpoint}/{file}"))
                        .map_err(|_| Failure::Security)?;
                    url.query_pairs_mut()
                        .append_pair("Signature", token)
                        .append_pair("file_id", &track.id.to_string())
                        .append_pair("user_id", owner)
                        .append_pair("platform", "tagcast-player")
                        .append_pair("version", env!("CARGO_PKG_VERSION"));
                    if !expires.is_empty() {
                        url.query_pairs_mut().append_pair("Expires", expires);
                    }
                    request(url)
                })
            }
            Location::Fixture(fixture) => {
                let url = if hls {
                    fixture.hls.as_ref().ok_or(Failure::Unsupported)?
                } else if quality != self.quality {
                    fixture.fallback.as_ref().unwrap_or(&fixture.direct)
                } else {
                    &fixture.direct
                };
                request(url.clone())
            }
        }
    }

    fn hint(&self, quality: Quality) -> Option<&str> {
        if let Location::Fixture(fixture) = &self.location {
            return Some(if quality != self.quality && quality == Quality::K128 {
                "mp3"
            } else {
                fixture.extension
            });
        }
        if quality != Quality::Original {
            return Some("mp3");
        }
        let Location::Native { track, .. } = &self.location else {
            return None;
        };
        match track.mime.as_str() {
            "audio/mpeg" => Some("mp3"),
            "audio/flac" | "audio/x-flac" => Some("flac"),
            "audio/mp4" | "audio/x-m4a" => Some("m4a"),
            "audio/aac" => Some("aac"),
            "audio/wav" | "audio/x-wav" => Some("wav"),
            "audio/ogg" => Some("ogg"),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Direct(Quality),
    Hls(Quality),
}

impl Route {
    pub fn first(quality: Quality) -> Self {
        if quality.hls() {
            Self::Hls(quality)
        } else {
            Self::Direct(quality)
        }
    }

    pub fn fallback(self, failure: &Failure) -> Option<Self> {
        match (self, failure) {
            (
                _,
                Failure::Cancelled | Failure::Auth | Failure::Security | Failure::SeekUnavailable,
            ) => None,
            (Self::Hls(quality), _) => Some(Self::Direct(quality)),
            (Self::Direct(Quality::Original), Failure::Unsupported | Failure::Http(404 | 415)) => {
                Some(Self::Direct(Quality::K128))
            }
            _ => None,
        }
    }
}

pub(crate) struct Input {
    pub source: Box<dyn MediaSource>,
    pub fault: Arc<Mutex<Option<Failure>>>,
    pub buffer: Option<Arc<Buffered>>,
    pub hint: Option<String>,
    pub origin: f64,
}

pub(crate) fn open(
    request: &MediaRequest,
    mode: Route,
    position: f64,
    stop: &CancellationToken,
) -> Result<Input> {
    let fault = Arc::new(Mutex::new(None));
    match mode {
        Route::Hls(quality) => {
            let (source, origin) = hls::Hls::open(request, stop, quality, position, fault.clone())?;
            Ok(Input {
                source: Box::new(source),
                fault,
                buffer: None,
                hint: Some("aac".into()),
                origin,
            })
        }
        Route::Direct(quality) => {
            let response = request.get(stop, quality, false, Some(0))?;
            let hint = request.hint(quality).map(str::to_owned);
            let length = if response.status().as_u16() == 206 {
                Some(media_range(&response, 0)?)
            } else {
                None
            };
            if let Some(length) = length {
                let range = RangeReader {
                    state: Mutex::new(RangeState {
                        response,
                        bytes: Default::default(),
                        consumed: 0,
                        position: 0,
                    }),
                    request: request.clone(),
                    quality,
                    stop: stop.clone(),
                    length,
                    fault: fault.clone(),
                };
                Ok(Input {
                    source: Box::new(range),
                    fault,
                    buffer: None,
                    hint,
                    origin: 0.0,
                })
            } else if response.status().as_u16() == 200 {
                let buffer = Buffered::start(&request.http, response, stop, fault.clone())?;
                let source = Box::new(BufferReader {
                    buffer: buffer.clone(),
                    position: 0,
                });
                Ok(Input {
                    source,
                    fault,
                    buffer: Some(buffer),
                    hint,
                    origin: 0.0,
                })
            } else {
                Err(Failure::Unsupported)
            }
        }
    }
}

fn resource(base: &Url, reference: &str) -> Result<Url> {
    if reference.is_empty()
        || reference.len() > 4096
        || reference.bytes().any(|c| c < 33 || c == b'\\')
    {
        return Err(Failure::Security);
    }
    let url = base.join(reference).map_err(|_| Failure::Security)?;
    let trusted = match url.host_str() {
        Some("streaming.ibroadcast.com") => true,
        Some(host) => host
            .strip_prefix("ib")
            .and_then(|p| p.strip_suffix(".ibroadcast.com"))
            .is_some_and(|n| n.len() == 3 && n.bytes().all(|c| c.is_ascii_digit())),
        None => false,
    };
    let eligible =
        (base.scheme() == "https" && url.scheme() == "https" && trusted && url.port().is_none())
            || (base.scheme() == "http"
                && base.host_str() == Some("127.0.0.1")
                && url.origin() == base.origin());
    if !eligible
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(Failure::Security);
    }
    Ok(url)
}

fn media_get(
    http: &Http,
    stop: &CancellationToken,
    mut url: Url,
    range: Option<u64>,
) -> Result<Response> {
    for _ in 0..=3 {
        let mut request = http
            .client
            .get(url.clone())
            .header(header::ACCEPT_ENCODING, "identity");
        if let Some(offset) = range {
            request = request.header(header::RANGE, format!("bytes={offset}-"));
        }
        let response = http.send(stop, request)?;
        if !response.status().is_redirection() {
            return check_status(response);
        }
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|h| h.to_str().ok())
            .ok_or(Failure::Unsupported)?;
        url = resource(&url, location)?;
    }
    Err(Failure::Unsupported)
}

fn media_range(response: &Response, requested: u64) -> Result<u64> {
    match response
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|h| h.to_str().ok())
    {
        Some(value) => {
            let (range, total) = value
                .strip_prefix("bytes ")
                .and_then(|v| v.split_once('/'))
                .ok_or(Failure::Unsupported)?;
            let (start, end) = range.split_once('-').ok_or(Failure::Unsupported)?;
            let (start, end, total) = (
                start.parse::<u64>(),
                end.parse::<u64>(),
                total.parse::<u64>(),
            );
            match (start, end, total) {
                (Ok(start), Ok(end), Ok(total))
                    if start == requested
                        && start <= end
                        && end.checked_add(1) == Some(total)
                        && response
                            .content_length()
                            .is_none_or(|length| length == total - start) =>
                {
                    Ok(total)
                }
                _ => Err(Failure::Unsupported),
            }
        }
        None => Err(Failure::Unsupported),
    }
}

fn move_to(position: u64, length: Option<u64>, offset: SeekFrom) -> io::Result<u64> {
    match offset {
        SeekFrom::Start(value) => Some(value),
        SeekFrom::Current(value) => position.checked_add_signed(value),
        SeekFrom::End(value) => length.and_then(|n| n.checked_add_signed(value)),
    }
    .ok_or_else(|| Failure::SeekUnavailable.into())
}

fn fault(error: Failure, slot: &Mutex<Option<Failure>>) -> io::Error {
    *slot.lock().unwrap() = Some(error.clone());
    error.into()
}

struct RangeReader {
    state: Mutex<RangeState>, // The response body is read by the decoder thread only.
    request: MediaRequest,
    quality: Quality,
    stop: CancellationToken,
    length: u64,
    fault: Arc<Mutex<Option<Failure>>>,
}

struct RangeState {
    response: Response,
    bytes: Box<[u8]>,
    consumed: usize,
    position: u64,
}

impl Read for RangeReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let state = self.state.get_mut().unwrap();
        if out.is_empty() || state.position == self.length {
            return Ok(0);
        }
        while state.consumed == state.bytes.len() {
            state.consumed = 0;
            state.bytes = match self.request.http.run(&self.stop, async {
                state.response.chunk().await.map_err(http_error)
            }) {
                Ok(Some(bytes)) => bytes.to_vec().into(),
                Ok(None) => return Err(fault(Failure::Network, &self.fault)),
                Err(error) => return Err(fault(error, &self.fault)),
            };
            if state.bytes.len() as u64 > self.length - state.position {
                return Err(fault(Failure::Unsupported, &self.fault));
            }
        }
        let bytes = &state.bytes[state.consumed..];
        let n = out.len().min(bytes.len());
        out[..n].copy_from_slice(&bytes[..n]);
        state.consumed += n;
        state.position += n as u64;
        Ok(n)
    }
}

impl Seek for RangeReader {
    fn seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
        let state = self.state.get_mut().unwrap();
        let target = move_to(state.position, Some(self.length), offset)?;
        if target > self.length {
            return Err(Failure::SeekUnavailable.into());
        }
        if target == state.position {
            return Ok(target);
        }
        if target == self.length {
            state.position = target;
            state.bytes = Default::default();
            state.consumed = 0;
            return Ok(target);
        }
        let response = self
            .request
            .get(&self.stop, self.quality, false, Some(target))
            .map_err(|error| fault(error, &self.fault))?;
        if response.status().as_u16() != 206
            || media_range(&response, target).ok() != Some(self.length)
        {
            return Err(fault(Failure::Unsupported, &self.fault));
        }
        state.response = response;
        state.bytes = Default::default();
        state.consumed = 0;
        state.position = target;
        Ok(target)
    }
}

impl MediaSource for RangeReader {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.length)
    }
}

pub(crate) struct Buffered {
    state: Mutex<BufferState>,
    event: Condvar,
    stop: CancellationToken,
}

struct BufferState {
    bytes: VecDeque<u8>,
    base: u64,
    read_head: u64,
    result: Option<Result<()>>,
}

impl Buffered {
    fn start(
        http: &Http,
        mut response: Response,
        parent: &CancellationToken,
        failure: Arc<Mutex<Option<Failure>>>,
    ) -> Result<Arc<Self>> {
        let buffer = Arc::new(Self {
            state: Mutex::new(BufferState {
                bytes: VecDeque::new(),
                base: 0,
                read_head: 0,
                result: None,
            }),
            event: Condvar::new(),
            stop: parent.child_token(),
        });
        let feed = buffer.clone();
        let http = http.clone();
        thread::Builder::new()
            .name("audio-buffer".into())
            .spawn(move || {
                let result: Result<()> = (|| {
                    while let Some(chunk) = http.run(&feed.stop, async {
                        response.chunk().await.map_err(http_error)
                    })? {
                        for bytes in chunk.chunks(64 * 1024) {
                            feed.push(bytes)?;
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = &result {
                    *failure.lock().unwrap() = Some(error.clone());
                }
                feed.state.lock().unwrap().result = Some(result);
                feed.event.notify_all();
            })
            .map_err(|_| Failure::Invalid("Could not start audio buffer"))?;
        Ok(buffer)
    }

    fn push(&self, bytes: &[u8]) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        while state.bytes.len() + bytes.len() > CACHE_BYTES {
            if self.stop.is_cancelled() {
                return Err(Failure::Cancelled);
            }
            let spare = state
                .read_head
                .saturating_sub(state.base)
                .saturating_sub(RETAIN_BYTES as u64) as usize;
            if spare >= bytes.len() {
                state.bytes.drain(..spare);
                state.base += spare as u64;
            } else {
                state = self
                    .event
                    .wait_timeout(state, Duration::from_millis(50))
                    .unwrap()
                    .0;
            }
        }
        state.bytes.extend(bytes);
        self.event.notify_all();
        Ok(())
    }

    fn length(&self) -> Option<u64> {
        let state = self.state.lock().unwrap();
        (state.base == 0 && matches!(state.result, Some(Ok(()))))
            .then_some(state.bytes.len() as u64)
    }

    pub(crate) fn reopen(self: &Arc<Self>) -> Option<Box<dyn MediaSource>> {
        self.length().map(|_| {
            Box::new(BufferReader {
                buffer: self.clone(),
                position: 0,
            }) as _
        })
    }

    pub(crate) fn cancel(&self) {
        self.stop.cancel();
        self.event.notify_all();
    }
}

struct BufferReader {
    buffer: Arc<Buffered>,
    position: u64,
}

impl Read for BufferReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let mut state = self.buffer.state.lock().unwrap();
        loop {
            if self.buffer.stop.is_cancelled() {
                return Err(Failure::Cancelled.into());
            }
            let offset = self
                .position
                .checked_sub(state.base)
                .ok_or(Failure::SeekUnavailable)? as usize;
            if offset < state.bytes.len() {
                let count = out.len().min(state.bytes.len() - offset);
                let (first, second) = state.bytes.as_slices();
                let n = count.min(first.len().saturating_sub(offset));
                if n > 0 {
                    out[..n].copy_from_slice(&first[offset..offset + n]);
                }
                if count > n {
                    let start = offset.saturating_sub(first.len());
                    out[n..count].copy_from_slice(&second[start..start + count - n]);
                }
                self.position += count as u64;
                state.read_head = state.read_head.max(self.position);
                self.buffer.event.notify_all();
                return Ok(count);
            }
            match &state.result {
                Some(Ok(())) => return Ok(0),
                Some(Err(error)) => return Err(error.clone().into()),
                None => {
                    state = self
                        .buffer
                        .event
                        .wait_timeout(state, Duration::from_millis(50))
                        .unwrap()
                        .0
                }
            }
        }
    }
}

impl Seek for BufferReader {
    fn seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
        let len = self.buffer.length();
        let target = move_to(self.position, len, offset)?;
        if target == self.position || len.is_some_and(|length| target <= length) {
            self.position = target;
            Ok(target)
        } else {
            Err(Failure::SeekUnavailable.into())
        }
    }
}

impl MediaSource for BufferReader {
    fn is_seekable(&self) -> bool {
        self.buffer.length().is_some()
    }
    fn byte_len(&self) -> Option<u64> {
        self.buffer.length()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_non_range_buffer_slides_without_exceeding_its_budget() {
        let buffer = Buffered {
            state: Mutex::new(BufferState {
                bytes: VecDeque::from(vec![0; CACHE_BYTES]),
                base: 0,
                read_head: (RETAIN_BYTES + 65536) as u64,
                result: None,
            }),
            event: Condvar::new(),
            stop: CancellationToken::new(),
        };
        buffer.push(&[1; 65536]).unwrap();
        let state = buffer.state.lock().unwrap();
        assert_eq!(state.base, 65536);
        assert_eq!(state.bytes.len(), CACHE_BYTES);
        assert_eq!(state.bytes.back(), Some(&1));
    }
}
