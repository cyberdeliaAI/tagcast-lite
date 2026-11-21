use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use cpal::{
    FromSample, SizedSample, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use symphonia::{
    core::{
        audio::SampleBuffer,
        codecs::{CODEC_TYPE_FLAC, CODEC_TYPE_NULL, Decoder, DecoderOptions},
        errors::Error as DecodeError,
        formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
        io::MediaSourceStream,
        meta::MetadataOptions,
        probe::Hint,
        units::{Time, TimeBase},
    },
    default::{get_codecs, get_probe},
};

use crate::{
    CancellationToken, Failure, Result,
    transport::{Input, MediaRequest, Quality, Route, open},
};

type Frame = [f32; 2];
const TICK: Duration = Duration::from_millis(5);
const CONVERT_BLOCK: usize = 512;

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Buffering,
    Playing,
    Recovering,
    Ended,
    Failed(Failure),
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub position: f64,
    pub duration: Option<f64>,
    pub paused: bool,
    pub phase: Phase,
    pub route: Option<Route>,
    pub note: Option<String>,
    pub first_audio: Option<Instant>,
    pub recoveries: usize,
    pub listened: f64,
    pub source_rate: Option<u32>,
    pub device_error: bool,
    pub track_key: u64,
    pub gapless_transition: bool,
}

#[derive(Clone)]
pub struct Output {
    state: Arc<Mutex<OutputState>>,
    pub rate: u32,
    capacity: usize,
}

struct OutputState {
    epoch: u64,
    frames: VecDeque<Frame>,
    played_frames: u64,
    source_rate: Option<u32>,
    track_key: u64,
    boundary: Option<Boundary>,
    gapless_transition: bool,
    position: f64,
    duration: Option<f64>,
    paused: bool,
    phase: Phase,
    route: Option<Route>,
    note: Option<String>,
    first_audio: Option<Instant>,
    recoveries: usize,
    volume: f32,
    eof: bool,
    priming: bool,
    device_error: bool,
}

struct Boundary {
    key: u64,
    duration: Option<f64>,
    remaining: usize,
}

impl Output {
    pub fn new(rate: u32) -> Self {
        let capacity = (rate / 4) as usize;
        let state = OutputState {
            epoch: 0,
            frames: VecDeque::with_capacity(capacity),
            played_frames: 0,
            source_rate: None,
            track_key: 0,
            boundary: None,
            gapless_transition: false,
            position: 0.0,
            duration: None,
            paused: false,
            phase: Phase::Idle,
            route: None,
            note: None,
            first_audio: None,
            recoveries: 0,
            volume: 0.8,
            eof: false,
            priming: true,
            device_error: false,
        };
        Self {
            state: Arc::new(Mutex::new(state)),
            rate,
            capacity,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let state = self.state.lock().unwrap();
        let phase = if state.device_error {
            Phase::Failed(Failure::Invalid("The audio output became unavailable"))
        } else {
            state.phase.clone()
        };
        Snapshot {
            position: state.position,
            duration: state.duration,
            paused: state.paused,
            phase,
            route: state.route,
            note: state.note.clone(),
            first_audio: state.first_audio,
            recoveries: state.recoveries,
            listened: state.played_frames as f64 / f64::from(self.rate),
            source_rate: state.source_rate,
            device_error: state.device_error,
            track_key: state.track_key,
            gapless_transition: state.gapless_transition,
        }
    }

    pub fn pause(&self, paused: bool) {
        self.state.lock().unwrap().paused = paused;
    }

    pub fn volume(&self) -> f32 {
        self.state.lock().unwrap().volume
    }

    pub fn set_volume(&self, volume: f32) -> f32 {
        let mut state = self.state.lock().unwrap();
        if volume.is_finite() {
            state.volume = volume.clamp(0.0, 1.0);
        }
        state.volume
    }

    pub fn adjust_volume(&self, amount: f32) -> f32 {
        let mut state = self.state.lock().unwrap();
        state.volume = (state.volume + amount).clamp(0.0, 1.0);
        state.volume
    }

    fn reset(
        &self,
        epoch: u64,
        key: u64,
        position: f64,
        paused: bool,
        phase: Phase,
        duration: Option<f64>,
    ) {
        let mut state = self.state.lock().unwrap();
        state.epoch = epoch;
        state.track_key = key;
        state.boundary = None;
        state.gapless_transition = false;
        state.frames.clear();
        state.played_frames = 0;
        state.source_rate = None;
        state.position = position;
        state.duration = duration;
        state.paused = paused;
        state.phase = phase;
        state.route = None;
        state.note = None;
        state.first_audio = None;
        state.recoveries = 0;
        state.eof = false;
        state.priming = true;
    }

    fn for_epoch(&self, epoch: u64, update: impl FnOnce(&mut OutputState)) {
        let mut state = self.state.lock().unwrap();
        if state.epoch == epoch {
            update(&mut state);
        }
    }

    fn push(&self, epoch: u64, frames: &[Frame]) -> usize {
        let mut n = 0;
        self.for_epoch(epoch, |state| {
            n = frames.len().min(self.capacity - state.frames.len());
            state.frames.extend(&frames[..n]);
        });
        n
    }

    fn cancel_boundary(&self, epoch: u64, key: u64) {
        self.for_epoch(epoch, |state| {
            if state
                .boundary
                .as_ref()
                .is_some_and(|boundary| boundary.key == key)
            {
                let boundary = state.boundary.take().unwrap();
                state.frames.truncate(boundary.remaining);
            }
        });
    }

    fn append_boundary(
        &self,
        epoch: u64,
        plan: &Successor,
        tail: usize,
        frames: &[Frame],
        duration: Option<f64>,
    ) -> Option<usize> {
        let mut state = self.state.lock().unwrap();
        let available = self.capacity - state.frames.len();
        if state.epoch != epoch
            || state.track_key != plan.from
            || state.frames.is_empty()
            || plan.stop.is_cancelled()
            || tail >= frames.len()
            || tail + (frames.len() - tail).min((self.rate / 40) as usize) > available
        {
            return None;
        }
        let n = available.min(frames.len());
        state.boundary = Some(Boundary {
            key: plan.key,
            duration,
            remaining: state.frames.len() + tail,
        });
        state.frames.extend(&frames[..n]);
        Some(n)
    }

    fn render(&self, frames: usize, delay: Duration, mut write: impl FnMut(usize, Frame)) -> usize {
        let Ok(mut state) = self.state.try_lock() else {
            return 0;
        };
        if state.device_error || state.paused {
            return 0;
        }
        if state.eof && state.frames.is_empty() {
            state.phase = Phase::Ended;
            return 0;
        }
        if state.priming && state.frames.len() < (self.rate / 50) as usize && !state.eof {
            return 0;
        }
        let count = state.frames.len().min(frames);
        if count > 0 {
            state.priming = false;
            state.phase = Phase::Playing;
            if state.first_audio.is_none() {
                state.first_audio = Some(Instant::now() + delay);
            }
            // Mark the track switch at the exact frame, including when a
            // single CoreAudio callback spans both tracks.
            let old_count = state
                .boundary
                .as_ref()
                .map_or(count, |boundary| boundary.remaining.min(count));
            if let Some(boundary) = &mut state.boundary {
                boundary.remaining -= old_count;
            }
            for index in 0..count {
                let [left, right] = state.frames.pop_front().unwrap();
                write(index, [left * state.volume, right * state.volume]);
            }
            if old_count < count {
                let boundary = state.boundary.take().unwrap();
                state.track_key = boundary.key;
                state.duration = boundary.duration;
                state.position = (count - old_count) as f64 / f64::from(self.rate);
                state.played_frames = (count - old_count) as u64;
                state.note = None;
                state.recoveries = 0;
                state.gapless_transition = true;
            } else {
                state.position += count as f64 / f64::from(self.rate);
                state.played_frames += count as u64;
            }
        }
        count
    }

    #[doc(hidden)]
    pub fn render_fixture(&self, data: &mut [Frame]) -> usize {
        data.fill([0.0; 2]);
        self.render(data.len(), Duration::ZERO, |i, frame| data[i] = frame)
    }

    pub fn device() -> std::result::Result<(Self, Stream), Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("No default audio output")?;
        let supported = device.default_output_config()?;
        let output = Self::new(supported.sample_rate().0);
        let config = supported.config();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => device_stream::<f32>(&device, &config, &output)?,
            cpal::SampleFormat::I16 => device_stream::<i16>(&device, &config, &output)?,
            cpal::SampleFormat::U16 => device_stream::<u16>(&device, &config, &output)?,
            cpal::SampleFormat::I32 => device_stream::<i32>(&device, &config, &output)?,
            _ => return Err("Unsupported output sample format".into()),
        };
        stream.play()?;
        Ok((output, stream))
    }
}

fn device_stream<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    output: &Output,
) -> std::result::Result<Stream, cpal::BuildStreamError> {
    let (output, errors) = (output.clone(), output.clone());
    let channels = usize::from(config.channels);
    device.build_output_stream(
        config,
        move |data: &mut [T], info| {
            data.fill(T::from_sample(0.0));
            let timing = info.timestamp();
            let delay = timing
                .playback
                .duration_since(&timing.callback)
                .unwrap_or_default();
            output.render(data.len() / channels, delay, |i, [left, right]| {
                let offset = i * channels;
                if channels == 1 {
                    data[offset] = T::from_sample((left + right) * 0.5);
                } else {
                    data[offset] = T::from_sample(left);
                    data[offset + 1] = T::from_sample(right);
                }
            });
        },
        move |_| {
            errors.state.lock().unwrap().device_error = true;
        },
        None,
    )
}

pub struct Player {
    pub output: Output,
    jobs: mpsc::Sender<Job>,
    thread: Option<thread::JoinHandle<()>>,
    stop: CancellationToken,
    seek: Arc<Mutex<Option<f64>>>,
    successor: Arc<Mutex<Option<Arc<Successor>>>>,
    epoch: u64,
}

struct Job {
    request: MediaRequest,
    key: u64,
    successor: Arc<Mutex<Option<Arc<Successor>>>>,
    stop: CancellationToken,
    seek: Arc<Mutex<Option<f64>>>,
    epoch: u64,
    position: f64,
}

pub struct NextTrack {
    pub from: u64,
    pub key: u64,
    pub request: MediaRequest,
    pub duration: Option<f64>,
}

struct Successor {
    from: u64,
    key: u64,
    stop: CancellationToken,
    request: MediaRequest,
    duration: Option<f64>,
    ready: Mutex<Option<Prepared>>,
}

struct Prepared {
    decoded: Decoded,
    frames: Vec<Frame>,
    rate: u32,
    channels: usize,
}

struct Active {
    key: u64,
    request: MediaRequest,
}

fn prepare_successor(plan: &Successor, device_rate: u32) -> Result<Prepared> {
    let input = open(
        &plan.request,
        Route::Direct(Quality::Original),
        0.0,
        &plan.stop,
    )?;
    if input.buffer.is_some() {
        if let Some(buffer) = input.buffer {
            buffer.cancel();
        }
        return Err(Failure::Unsupported);
    }
    let mut decoded = Decoded::start(input, 0.0, device_rate, false)?;
    if !decoded.flac {
        return Err(Failure::Unsupported);
    }
    let mut frames = Vec::new();
    let Some((rate, channels)) = decoded.next_raw(&mut frames)? else {
        return Err(Failure::Unsupported);
    };
    let min = (rate / 20).max(1024) as usize;
    while frames.len() < min && !plan.stop.is_cancelled() {
        if decoded.next_raw(&mut frames)? != Some((rate, channels)) {
            return Err(Failure::Unsupported);
        }
    }
    if channels > 2 || frames.len() > 65_536 {
        return Err(Failure::Unsupported);
    }
    Ok(Prepared {
        decoded,
        frames,
        rate,
        channels,
    })
}

impl Player {
    pub fn new(output: Output) -> std::io::Result<Self> {
        let (jobs, rx) = mpsc::channel::<Job>();
        let sink = output.clone();
        let worker = thread::Builder::new()
            .name("audio-decoder".into())
            .spawn(move || {
                while let Ok(mut job) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        job = newer;
                    }
                    if !job.stop.is_cancelled() {
                        run(&job, &sink);
                    }
                }
            })?;
        Ok(Self {
            output,
            jobs,
            thread: Some(worker),
            stop: CancellationToken::new(),
            seek: Arc::default(),
            successor: Arc::default(),
            epoch: 0,
        })
    }

    pub fn play(
        &mut self,
        request: MediaRequest,
        position: f64,
        duration: Option<f64>,
        paused: bool,
    ) {
        self.play_track(0, request, position, duration, paused);
    }

    pub fn play_track(
        &mut self,
        key: u64,
        request: MediaRequest,
        position: f64,
        duration: Option<f64>,
        paused: bool,
    ) {
        self.stop.cancel();
        self.epoch = self.epoch.wrapping_add(1);
        self.stop = CancellationToken::new();
        self.seek = Arc::default();
        self.successor = Arc::default();
        self.output.reset(
            self.epoch,
            key,
            position,
            paused,
            Phase::Buffering,
            duration,
        );
        let _ = self.jobs.send(Job {
            epoch: self.epoch,
            request,
            key,
            successor: self.successor.clone(),
            stop: self.stop.clone(),
            seek: self.seek.clone(),
            position,
        });
    }

    pub fn stop(&mut self) {
        self.stop.cancel();
        self.epoch = self.epoch.wrapping_add(1);
        self.output
            .reset(self.epoch, 0, 0.0, false, Phase::Idle, None);
    }

    pub fn plan_successor(&self, next: Option<NextTrack>) {
        let mut slot = self.successor.lock().unwrap();
        if slot
            .as_ref()
            .zip(next.as_ref())
            .is_some_and(|(old, new)| old.from == new.from && old.key == new.key)
        {
            return;
        }
        if let Some(old) = slot.take() {
            self.output.for_epoch(self.epoch, |state| {
                if state.track_key == old.key {
                    return;
                }
                old.stop.cancel();
                if let Some(boundary) = state.boundary.take() {
                    state.frames.truncate(boundary.remaining);
                }
            });
        }
        let snapshot = self.output.snapshot();
        let Some(next) = next else {
            return;
        };
        if snapshot.track_key != next.from
            || snapshot.first_audio.is_none()
            || snapshot.phase != Phase::Playing
            || snapshot.route != Some(Route::Direct(Quality::Original))
            || next.request.quality != Quality::Original
        {
            return;
        }
        let plan = Arc::new(Successor {
            from: next.from,
            key: next.key,
            request: next.request,
            duration: next.duration,
            stop: self.stop.child_token(),
            ready: Mutex::new(None),
        });
        *slot = Some(plan.clone());
        let device_rate = self.output.rate;
        let _ = thread::Builder::new()
            .name("album-lookahead".into())
            .spawn(move || {
                if let Ok(prepared) = prepare_successor(&plan, device_rate)
                    && !plan.stop.is_cancelled()
                {
                    *plan.ready.lock().unwrap() = Some(prepared);
                }
            });
    }

    pub fn seek(&self, position: f64) {
        if position.is_finite() {
            *self.seek.lock().unwrap() = Some(position.max(0.0));
            self.plan_successor(None);
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.cancel();
        let (replacement, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.jobs, replacement));
        if let Some(worker) = self.thread.take() {
            let _ = worker.join();
        }
    }
}

fn run(job: &Job, out: &Output) {
    let mut active = Active {
        key: job.key,
        request: job.request.clone(),
    };
    let mut route = Route::first(active.request.quality);
    let mut position = job.position;
    let mut network_used = false;
    loop {
        out.for_epoch(job.epoch, |state| state.route = Some(route));
        let attempt = job.stop.child_token();
        let before = active.key;
        let result = play_attempt(job, out, &mut active, route, position, &attempt);
        if active.key != before {
            network_used = false;
        }
        attempt.cancel();
        let Err(failure) = result else {
            return;
        };
        if job.stop.is_cancelled() || failure == Failure::Cancelled {
            return;
        }

        let replacement = route.fallback(&failure);
        let retry = replacement.is_some() || (failure.temporary() && !network_used);
        out.for_epoch(job.epoch, |state| {
            state.frames.clear();
            state.boundary = None;
            state.eof = false;
            state.priming = true;
            position = state.position;
            if retry {
                state.phase = Phase::Recovering;
                state.recoveries += 1;
                state.note = Some(match replacement {
                    Some(Route::Direct(q)) => format!("Using direct {} stream", q.value()),
                    _ => "Restoring the current track after a network interruption".into(),
                });
            } else {
                state.phase = Phase::Failed(failure.clone());
            }
        });
        if !retry {
            return;
        }
        if let Some(next) = replacement {
            route = next;
        }
        // HLS fallback is part of the same transient recovery budget.
        if failure.temporary() {
            network_used = true;
        }
    }
}

fn play_attempt(
    job: &Job,
    out: &Output,
    active: &mut Active,
    route: Route,
    position: f64,
    stop: &CancellationToken,
) -> Result<()> {
    let mut decoded = Decoded::start(
        open(&active.request, route, position, stop)?,
        position,
        out.rate,
        true,
    )?;
    out.for_epoch(job.epoch, |state| {
        if decoded.duration.is_some() {
            state.duration = decoded.duration;
        }
    });
    let mut pending = Vec::new();
    let mut raw = Vec::new();
    let mut sent = 0;
    let mut ending = false;
    let mut any_audio = false;
    while !stop.is_cancelled() {
        let target = job.seek.lock().unwrap().take();
        if let Some(target) = target {
            let seek_stop = stop.child_token();
            let next_position = out
                .snapshot()
                .duration
                .map_or(target, |end| target.min((end - 0.05).max(0.0)));
            let next = match &decoded.buffer {
                Some(buffer) => {
                    buffer
                        .reopen()
                        .ok_or(Failure::SeekUnavailable)
                        .and_then(|source| {
                            Decoded::start(
                                Input {
                                    source,
                                    fault: decoded.fault.clone(),
                                    buffer: Some(buffer.clone()),
                                    hint: decoded.hint.clone(),
                                    origin: 0.0,
                                },
                                next_position,
                                out.rate,
                                false,
                            )
                        })
                }
                None => open(&active.request, route, next_position, &seek_stop)
                    .and_then(|input| Decoded::start(input, next_position, out.rate, false)),
            };
            match next {
                Ok(next) => {
                    out.for_epoch(job.epoch, |state| {
                        state.frames.clear();
                        state.position = next_position;
                        state.eof = false;
                        state.priming = true;
                        state.note = None;
                        state.phase = Phase::Buffering;
                    });
                    decoded = next;
                    pending.clear();
                    sent = 0;
                    ending = false;
                    any_audio = false;
                }
                Err(error) => {
                    seek_stop.cancel();
                    out.for_epoch(job.epoch, |state| state.note = Some(error.to_string()));
                }
            }
        }

        if sent < pending.len() {
            sent += out.push(job.epoch, &pending[sent..]);
            if sent < pending.len() {
                thread::sleep(TICK);
            }
            continue;
        }
        if ending {
            out.for_epoch(job.epoch, |state| state.eof = true);
            if out.snapshot().phase == Phase::Ended {
                return Ok(());
            }
            thread::sleep(TICK);
            continue;
        }
        pending.clear();
        sent = 0;
        raw.clear();
        if let Some((rate, channels)) = decoded.next_raw(&mut raw)? {
            decoded.channels = channels;
            decoded
                .converter
                .convert(rate, raw.iter().copied(), &mut pending)?;
        } else {
            let candidate = job
                .successor
                .lock()
                .unwrap()
                .as_ref()
                .filter(|plan| plan.from == active.key && !plan.stop.is_cancelled())
                .cloned();
            let prepared = candidate
                .as_ref()
                .and_then(|plan| plan.ready.lock().unwrap().take());
            if let Some((plan, mut next)) = candidate.zip(prepared)
                && decoded.flac
                && decoded.channels == next.channels
                && decoded.converter.source_rate == next.rate
                && route == Route::Direct(Quality::Original)
                && any_audio
                && job.seek.lock().unwrap().is_none()
            {
                let tail = decoded.converter.tail_frames();
                // Keep the converter and its filter history across the album
                // boundary. Only the upcoming track's first packet is held.
                decoded
                    .converter
                    .convert(next.rate, next.frames.drain(..), &mut pending)?;
                let duration = next.decoded.duration.or(plan.duration);
                let pushed = loop {
                    if let Some(n) = out.append_boundary(job.epoch, &plan, tail, &pending, duration)
                    {
                        break Some(n);
                    }
                    let retry = out.state.lock().unwrap().frames.len() > tail;
                    if !retry
                        || stop.is_cancelled()
                        || plan.stop.is_cancelled()
                        || job.seek.lock().unwrap().is_some()
                        || pending.len() <= tail
                    {
                        break None;
                    }
                    thread::sleep(TICK);
                };
                if let Some(n) = pushed {
                    while !stop.is_cancelled() {
                        let state = out.snapshot();
                        if state.track_key == plan.key {
                            active.key = plan.key;
                            active.request = plan.request.clone();
                            next.decoded.converter =
                                std::mem::replace(&mut decoded.converter, Converter::new(out.rate));
                            next.decoded.channels = next.channels;
                            decoded = next.decoded;
                            sent = n;
                            any_audio = true;
                            break;
                        }
                        if plan.stop.is_cancelled() || job.seek.lock().unwrap().is_some() {
                            out.cancel_boundary(job.epoch, plan.key);
                            pending.clear();
                            ending = true;
                            break;
                        }
                        thread::sleep(TICK);
                    }
                    continue;
                }
                // The boundary was edited or the buffer already drained.
                // Retain only the old track's resampler tail.
                pending.truncate(tail);
            } else {
                decoded.converter.finish(&mut pending)?;
            }
            if !any_audio && pending.is_empty() {
                return Err(Failure::Unsupported);
            }
            ending = true;
        }
        if !pending.is_empty() && !any_audio {
            out.for_epoch(job.epoch, |state| {
                state.source_rate = Some(decoded.converter.source_rate)
            });
        }
        any_audio |= !pending.is_empty();
    }
    Err(Failure::Cancelled)
}

struct Decoded {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    flac: bool,
    channels: usize,
    track: u32,
    time_base: Option<TimeBase>,
    target: f64,
    discard_deadline: Instant,
    duration: Option<f64>,
    decoder_pos: f64,
    fault: Arc<Mutex<Option<Failure>>>,
    buffer: Option<Arc<crate::transport::Buffered>>,
    hint: Option<String>,
    sample_buffer: Option<SampleBuffer<f32>>,
    converter: Converter,
}

impl Decoded {
    fn start(input: Input, position: f64, device_rate: u32, may_discard: bool) -> Result<Self> {
        let seekable = input.source.is_seekable();
        let mut hint = Hint::new();
        if let Some(extension) = &input.hint {
            hint.with_extension(extension);
        }
        let mut probe = get_probe()
            .format(
                &hint,
                MediaSourceStream::new(input.source, Default::default()),
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .map_err(|_| {
                input
                    .fault
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or(Failure::Unsupported)
            })?;
        let track = probe
            .format
            .default_track()
            .filter(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or(Failure::Unsupported)?;
        let (id, time_base) = (track.id, track.codec_params.time_base);
        let flac = track.codec_params.codec == CODEC_TYPE_FLAC;
        let duration = track
            .codec_params
            .n_frames
            .zip(time_base)
            .map(|(n, base)| seconds(base.calc_time(n)) + input.origin)
            .filter(|_| input.origin == 0.0 && input.hint.as_deref() != Some("aac"));
        let mut decoder = get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|_| Failure::Unsupported)?;
        let target = (position - input.origin).max(0.0);
        if target > 0.0 && seekable {
            let time = Time::new(target.trunc() as u64, target.fract());
            let seek = probe.format.seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(id),
                },
            );
            if let Err(error) = seek {
                return Err(input
                    .fault
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| decode_seek_error(error)));
            }
            decoder.reset();
        } else if target > 0.0 && !may_discard && input.hint.as_deref() != Some("aac") {
            if let Some(buffer) = &input.buffer {
                buffer.cancel();
            }
            return Err(Failure::SeekUnavailable);
        }
        Ok(Self {
            format: probe.format,
            decoder,
            flac,
            channels: 0,
            track: id,
            time_base,
            target,
            duration,
            decoder_pos: 0.0,
            discard_deadline: Instant::now() + Duration::from_secs(12),
            fault: input.fault,
            buffer: input.buffer,
            hint: input.hint,
            sample_buffer: None,
            converter: Converter::new(device_rate),
        })
    }

    fn next_raw(&mut self, out: &mut Vec<Frame>) -> Result<Option<(u32, usize)>> {
        let mut rejected = 0;
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(error) => {
                    if let Some(failure) = self.fault.lock().unwrap().clone() {
                        return Err(failure);
                    }
                    if end_of_stream(&error) {
                        return Ok(None);
                    }
                    return Err(Failure::Unsupported);
                }
            };
            if packet.track_id() != self.track {
                continue;
            }
            let audio = match self.decoder.decode(&packet) {
                Ok(audio) => audio,
                Err(_) => {
                    rejected += 1;
                    if rejected >= 6 {
                        return Err(Failure::Unsupported);
                    }
                    continue;
                }
            };
            let spec = *audio.spec();
            let channels = spec.channels.count();
            if channels == 0 || channels > 8 || !(8000..=384000).contains(&spec.rate) {
                return Err(Failure::Unsupported);
            }
            let time = self.time_base.map_or(self.decoder_pos, |base| {
                seconds(base.calc_time(packet.ts()))
            });
            self.decoder_pos = time + audio.frames() as f64 / f64::from(spec.rate);
            let skip = ((self.target - time) * f64::from(spec.rate))
                .round()
                .max(0.0) as usize;
            if skip >= audio.frames() {
                if Instant::now() >= self.discard_deadline {
                    return Err(Failure::SeekUnavailable);
                }
                continue;
            }
            self.target = 0.0;
            let samples = self
                .sample_buffer
                .get_or_insert_with(|| SampleBuffer::new(audio.capacity() as u64, spec));
            if samples.capacity() < audio.capacity() * channels {
                *samples = SampleBuffer::new(audio.capacity() as u64, spec);
            }
            samples.copy_interleaved_ref(audio);
            let frames = samples
                .samples()
                .chunks_exact(channels)
                .skip(skip)
                .map(|frame| {
                    if channels == 1 {
                        return [frame[0]; 2];
                    }
                    if channels == 2 {
                        return [frame[0], frame[1]];
                    }
                    let center = frame[2..].iter().sum::<f32>() * 0.5 / (channels - 2) as f32;
                    [frame[0] * 0.7 + center, frame[1] * 0.7 + center]
                });
            out.extend(frames);
            return Ok(Some((spec.rate, channels)));
        }
    }
}

fn end_of_stream(error: &DecodeError) -> bool {
    matches!(error, DecodeError::IoError(err) if err.kind() == std::io::ErrorKind::UnexpectedEof)
}

fn decode_seek_error(error: DecodeError) -> Failure {
    match error {
        DecodeError::IoError(err) => err
            .get_ref()
            .and_then(|err| err.downcast_ref::<Failure>())
            .cloned()
            .unwrap_or(Failure::SeekUnavailable),
        _ => Failure::SeekUnavailable,
    }
}

fn seconds(time: Time) -> f64 {
    time.seconds as f64 + time.frac
}

struct Converter {
    target_rate: u32,
    source_rate: u32,
    inputs: [Vec<f32>; 2],
    outputs: Vec<Vec<f32>>,
    resampler: Option<SincFixedIn<f32>>,
    delay: usize,
    received: usize,
    produced: usize,
}

impl Converter {
    fn new(target_rate: u32) -> Self {
        Self {
            target_rate,
            source_rate: 0,
            inputs: std::array::from_fn(|_| Vec::with_capacity(CONVERT_BLOCK)),
            outputs: Vec::new(),
            resampler: None,
            delay: 0,
            received: 0,
            produced: 0,
        }
    }

    fn convert(
        &mut self,
        rate: u32,
        frames: impl Iterator<Item = Frame>,
        out: &mut Vec<Frame>,
    ) -> Result<()> {
        if self.source_rate == 0 {
            self.source_rate = rate;
            if rate != self.target_rate {
                let params = SincInterpolationParameters {
                    sinc_len: 128,
                    f_cutoff: 0.93,
                    interpolation: SincInterpolationType::Linear,
                    oversampling_factor: 64,
                    window: WindowFunction::BlackmanHarris2,
                };
                let resampler = SincFixedIn::new(
                    f64::from(self.target_rate) / f64::from(rate),
                    1.0,
                    params,
                    CONVERT_BLOCK,
                    2,
                )
                .map_err(|_| Failure::Unsupported)?;
                self.delay = resampler.output_delay();
                self.outputs = resampler.output_buffer_allocate(true);
                self.resampler = Some(resampler);
            }
        }
        if self.source_rate != rate {
            return Err(Failure::Unsupported);
        }
        if self.resampler.is_none() {
            out.extend(frames);
            return Ok(());
        }
        for [left, right] in frames {
            self.inputs[0].push(left);
            self.inputs[1].push(right);
            self.received += 1;
            if self.inputs[0].len() == CONVERT_BLOCK {
                self.process(out)?;
            }
        }
        Ok(())
    }

    fn process(&mut self, out: &mut Vec<Frame>) -> Result<()> {
        let resampler = self.resampler.as_mut().unwrap();
        let (_, count) = resampler
            .process_into_buffer(&self.inputs, &mut self.outputs, None)
            .map_err(|_| Failure::Unsupported)?;
        for buffer in &mut self.inputs {
            buffer.clear();
        }
        let skip = self.delay.min(count);
        self.delay -= skip;
        let goal = (self.received as f64 * f64::from(self.target_rate)
            / f64::from(self.source_rate))
        .round() as usize;
        let take = (count - skip).min(goal - self.produced);
        out.extend(
            (skip..skip + take).map(|index| [self.outputs[0][index], self.outputs[1][index]]),
        );
        self.produced += take;
        Ok(())
    }

    fn tail_frames(&self) -> usize {
        if self.resampler.is_none() {
            return 0;
        }
        let expected = (self.received as f64 * f64::from(self.target_rate)
            / f64::from(self.source_rate))
        .round() as usize;
        expected.saturating_sub(self.produced)
    }

    fn finish(&mut self, out: &mut Vec<Frame>) -> Result<()> {
        if self.resampler.is_some() {
            // Pad both channels explicitly. A zero-length input channel is
            // invalid to the resampler, even when only its tail is needed.
            for _ in 0..2 {
                for input in &mut self.inputs {
                    input.resize(CONVERT_BLOCK, 0.0);
                }
                self.process(out)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Url, account::Http};

    fn successor() -> Successor {
        let request = MediaRequest::fixture(
            Http::new().unwrap(),
            Url::parse("http://127.0.0.1/a.flac").unwrap(),
            None,
            None,
            "flac",
            Quality::Original,
        )
        .unwrap();
        Successor {
            from: 10,
            key: 11,
            stop: CancellationToken::new(),
            request,
            duration: Some(16.0),
            ready: Mutex::new(None),
        }
    }

    #[test]
    fn callback_changes_track_at_the_exact_sub_buffer_boundary() {
        let out = Output::new(48_000);
        out.reset(1, 10, 1.0, false, Phase::Playing, None);
        out.for_epoch(1, |state| state.priming = false);
        out.push(1, &[[0.25, 0.25]; 3]);
        let plan = successor();
        assert_eq!(
            out.append_boundary(1, &plan, 0, &[[0.5, 0.5]; 2], Some(16.0)),
            Some(2)
        );
        let mut samples = [[0.0, 0.0]; 5];
        assert_eq!(out.render_fixture(&mut samples), 5);
        assert_eq!(out.snapshot().track_key, 11);
        assert_eq!(out.snapshot().position, 2.0 / 48_000.0);
        assert_eq!(samples[..3], [[0.2, 0.2]; 3]);
        assert_eq!(samples[3..], [[0.4, 0.4]; 2]);
    }

    #[test]
    fn cancelling_a_boundary_discards_only_the_successor_samples() {
        let out = Output::new(48_000);
        out.reset(1, 10, 1.0, false, Phase::Playing, None);
        out.for_epoch(1, |state| state.priming = false);
        out.push(1, &[[0.25, 0.25]; 3]);
        let plan = successor();
        out.append_boundary(1, &plan, 0, &[[0.5, 0.5]; 2], None);
        plan.stop.cancel();
        out.cancel_boundary(1, 11);
        let mut samples = [[0.0, 0.0]; 5];
        assert_eq!(out.render_fixture(&mut samples), 3);
        assert_eq!(out.snapshot().track_key, 10);
        assert_eq!(samples[..3], [[0.2, 0.2]; 3]);
        assert_eq!(samples[3..], [[0.0, 0.0]; 2]);
    }
}
