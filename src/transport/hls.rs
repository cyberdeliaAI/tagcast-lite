use std::{
    io::{self, Cursor, Read, Seek, SeekFrom},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use m3u8_rs::{MediaPlaylist, Playlist};
use mpeg2ts_reader::{
    StreamType, demultiplex as ts,
    packet::Pid,
    packet_filter_switch,
    pes::{ElementaryStreamConsumer, PesContents, PesHeader, PesPacketFilter},
};
use symphonia::core::io::MediaSource;

use super::{MediaRequest, Quality, fault, media_get, resource};
use crate::{CancellationToken, Failure, Result, Url, account::Http};

const PLAYLIST_LIMIT: usize = 512 * 1024;
const SEGMENT_LIMIT: usize = 4 * 1024 * 1024;

struct Segments {
    playlist: MediaPlaylist,
    url: Url,
    index: usize,
    data: Cursor<Vec<u8>>,
}

pub(super) struct Hls {
    // Only one decoder reads this source. Mutex supplies the MediaSource Sync bound.
    segments: Mutex<Segments>,
    http: Http,
    stop: CancellationToken,
    fault: Arc<Mutex<Option<Failure>>>,
    bytes_read: u64,
}

impl Hls {
    pub fn open(
        request: &MediaRequest,
        stop: &CancellationToken,
        quality: Quality,
        position: f64,
        fault: Arc<Mutex<Option<Failure>>>,
    ) -> Result<(Self, f64)> {
        let response = request.get(stop, quality, true, None)?;
        let mut url = response.url().clone();
        let mut playlist = match parse(&request.http, stop, response)? {
            Playlist::MediaPlaylist(playlist) => playlist,
            Playlist::MasterPlaylist(master) => {
                let goal = quality.value().parse::<u64>().unwrap_or(128) * 1000;
                let variant = master
                    .variants
                    .iter()
                    .filter(|v| !v.is_i_frame && v.audio.is_none())
                    .min_by_key(|v| v.average_bandwidth.unwrap_or(v.bandwidth).abs_diff(goal))
                    .filter(|v| {
                        v.average_bandwidth.unwrap_or(v.bandwidth).abs_diff(goal) <= goal * 15 / 100
                    })
                    .ok_or(Failure::Unsupported)?;
                url = resource(&url, &variant.uri)?;
                load(&request.http, stop, &url)?
            }
        };
        validate(&playlist)?;
        let deadline = Instant::now() + Duration::from_millis(2500);
        while !playlist.end_list
            && position == 0.0
            && playlist.segments.len() < 2
            && Instant::now() < deadline
        {
            wait(&request.http, stop, Duration::from_millis(350))?;
            let newer = load(&request.http, stop, &url)?;
            check_growth(&playlist, &newer)?;
            playlist = newer;
        }
        let mut origin = 0.0;
        let index = playlist
            .segments
            .iter()
            .position(|s| {
                let end = origin + f64::from(s.duration);
                if end > position {
                    true
                } else {
                    origin = end;
                    false
                }
            })
            .ok_or(if position > 0.0 {
                Failure::SeekUnavailable
            } else {
                Failure::Network
            })?;
        Ok((
            Self {
                segments: Mutex::new(Segments {
                    playlist,
                    url,
                    index,
                    data: Cursor::default(),
                }),
                http: request.http.clone(),
                stop: stop.clone(),
                fault,
                bytes_read: 0,
            },
            origin,
        ))
    }
}

fn parse(http: &Http, stop: &CancellationToken, response: reqwest::Response) -> Result<Playlist> {
    let data = http.body(stop, response, PLAYLIST_LIMIT, 10)?;
    m3u8_rs::parse_playlist_res(&data).map_err(|_| Failure::Unsupported)
}

fn load(http: &Http, stop: &CancellationToken, url: &Url) -> Result<MediaPlaylist> {
    let response = media_get(http, stop, url.clone(), None)?;
    match parse(http, stop, response)? {
        Playlist::MediaPlaylist(playlist) => {
            validate(&playlist)?;
            Ok(playlist)
        }
        _ => Err(Failure::Unsupported),
    }
}

fn validate(playlist: &MediaPlaylist) -> Result<()> {
    if playlist.segments.len() > 4096
        || playlist.i_frames_only
        || playlist.start.is_some()
        || (!playlist.end_list
            && !matches!(
                playlist.playlist_type,
                Some(m3u8_rs::MediaPlaylistType::Event)
            ))
        || playlist.segments.iter().any(|s| {
            s.discontinuity
                || s.key.is_some()
                || s.map.is_some()
                || s.byte_range.is_some()
                || !s.duration.is_finite()
                || !(0.0..=90.0).contains(&s.duration)
                || s.duration == 0.0
        })
    {
        return Err(Failure::Unsupported);
    }
    Ok(())
}

fn check_growth(old: &MediaPlaylist, new: &MediaPlaylist) -> Result<()> {
    if new.media_sequence != old.media_sequence || new.segments.len() < old.segments.len() {
        return Err(Failure::Unsupported);
    }
    Ok(())
}

fn wait(http: &Http, stop: &CancellationToken, duration: Duration) -> Result<()> {
    http.run(stop, async {
        tokio::time::sleep(duration).await;
        Ok(())
    })
}

fn next_segment(
    http: &Http,
    stop: &CancellationToken,
    stream: &mut Segments,
) -> Result<Option<Vec<u8>>> {
    let deadline = Instant::now() + Duration::from_secs(8);
    while stream.index == stream.playlist.segments.len() && !stream.playlist.end_list {
        if Instant::now() >= deadline {
            return Err(Failure::Network);
        }
        let period = (stream.playlist.target_duration.saturating_mul(500)).clamp(350, 2000);
        wait(http, stop, Duration::from_millis(period))?;
        let newer = load(http, stop, &stream.url)?;
        check_growth(&stream.playlist, &newer)?;
        stream.playlist = newer;
    }
    let Some(segment) = stream.playlist.segments.get(stream.index) else {
        return Ok(None);
    };
    let url = resource(&stream.url, &segment.uri)?;
    let response = media_get(http, stop, url, None)?;
    let data = http.body(stop, response, SEGMENT_LIMIT, 10)?;
    let audio = adts(&data)?;
    stream.index += 1;
    Ok(Some(audio))
}

impl Read for Hls {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let stream = self.segments.get_mut().unwrap();
        if stream.data.position() == stream.data.get_ref().len() as u64 {
            let next = next_segment(&self.http, &self.stop, stream)
                .map_err(|err| fault(err, &self.fault))?;
            let Some(audio) = next else {
                return Ok(0);
            };
            stream.data = Cursor::new(audio);
        }
        let n = stream.data.read(out)?;
        self.bytes_read += n as u64;
        Ok(n)
    }
}

impl Seek for Hls {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        if target == SeekFrom::Current(0) {
            Ok(self.bytes_read)
        } else {
            Err(Failure::SeekUnavailable.into())
        }
    }
}

impl MediaSource for Hls {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

packet_filter_switch! {
    Filter<Demux> {
        Pat: ts::PatPacketFilter<Demux>,
        Pmt: ts::PmtPacketFilter<Demux>,
        Audio: PesPacketFilter<Demux, Audio>,
        Ignore: ts::NullPacketFilter<Demux>,
    }
}

#[derive(Default)]
pub struct Demux {
    changes: ts::FilterChangeset<Filter>,
    audio: Vec<u8>,
    found_audio: bool,
    damaged: bool,
}

impl ts::DemuxContext for Demux {
    type F = Filter;

    fn filter_changeset(&mut self) -> &mut ts::FilterChangeset<Filter> {
        &mut self.changes
    }

    fn construct(&mut self, request: ts::FilterRequest<'_, '_>) -> Filter {
        match request {
            ts::FilterRequest::ByPid(Pid::PAT) => Filter::Pat(ts::PatPacketFilter::default()),
            ts::FilterRequest::Pmt {
                pid,
                program_number,
            } => Filter::Pmt(ts::PmtPacketFilter::new(pid, program_number)),
            ts::FilterRequest::ByStream {
                stream_type: StreamType::ADTS,
                ..
            } if !self.found_audio => {
                self.found_audio = true;
                Filter::Audio(PesPacketFilter::new(Audio))
            }
            _ => Filter::Ignore(ts::NullPacketFilter::default()),
        }
    }
}

pub struct Audio;

impl ElementaryStreamConsumer<Demux> for Audio {
    fn start_stream(&mut self, _: &mut Demux) {}
    fn begin_packet(&mut self, ctx: &mut Demux, header: PesHeader<'_>) {
        let PesContents::Parsed(Some(parsed)) = header.contents() else {
            ctx.damaged = true;
            return;
        };
        self.continue_packet(ctx, parsed.payload());
    }
    fn continue_packet(&mut self, ctx: &mut Demux, bytes: &[u8]) {
        ctx.audio.extend_from_slice(bytes);
    }
    fn end_packet(&mut self, _: &mut Demux) {}
    fn continuity_error(&mut self, ctx: &mut Demux) {
        ctx.damaged = true;
    }
}

fn adts(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 188 || !data.len().is_multiple_of(188) || data[0] != 0x47 {
        return Err(Failure::Unsupported);
    }
    let mut context = Demux::default();
    let mut reader = ts::Demultiplex::new(&mut context);
    reader.push(&mut context, data);
    if context.damaged
        || context.audio.len() < 7
        || context.audio[0] != 0xff
        || context.audio[1] & 0xf6 != 0xf0
    {
        return Err(Failure::Unsupported);
    }
    Ok(context.audio)
}
