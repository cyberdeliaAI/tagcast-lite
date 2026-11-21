use std::{
    error::Error,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tagcast_player::{
    Url,
    account::Http,
    audio::{NextTrack, Output, Phase, Player, Snapshot},
    transport::{MediaRequest, Quality, Route},
};

const RATE: u32 = 48_000;
const BLOCK: usize = 240;

struct Checks {
    http: Http,
    base: Url,
    player: Player,
    out: Output,
}

impl Checks {
    fn new(base: Url, out: Output) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            http: Http::new()?,
            base,
            player: Player::new(out.clone())?,
            out,
        })
    }

    fn play(
        &mut self,
        path: &str,
        hls: Option<&str>,
        fallback: Option<&str>,
        ext: &'static str,
        q: Quality,
        paused: bool,
    ) {
        let url = |name| self.base.join(name).unwrap();
        let request = MediaRequest::fixture(
            self.http.clone(),
            url(path),
            hls.map(url),
            fallback.map(url),
            ext,
            q,
        )
        .unwrap();
        self.player.play(request, 0.0, Some(16.0), paused);
    }

    fn tick(&self, deadline: Instant, wait: bool, data: &mut Vec<f32>) -> Snapshot {
        assert!(
            Instant::now() < deadline,
            "Timed out at {:?}",
            self.out.snapshot()
        );
        let mut pcm = [[0.0; 2]; BLOCK];
        let len = self.out.render_fixture(&mut pcm);
        data.extend(pcm[..len].iter().map(|frame| frame[0]));
        if wait {
            thread::sleep(Duration::from_millis(5));
        } else if len == 0 {
            thread::sleep(Duration::from_millis(1));
        }
        self.out.snapshot()
    }

    fn hear(&self, sample_count: usize, wait: bool) -> (Snapshot, Vec<f32>) {
        let mut data = Vec::with_capacity(sample_count + BLOCK);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let state = self.tick(deadline, wait, &mut data);
            assert!(
                !matches!(state.phase, Phase::Failed(_) | Phase::Ended),
                "Failed before PCM: {state:?}"
            );
            if data.len() >= sample_count {
                return (state, data);
            }
        }
    }

    fn seek(&self, target: f64, sample_count: usize) -> Vec<f32> {
        self.out.pause(true);
        self.player.seek(target);
        until(8, || {
            let state = self.out.snapshot();
            (state.position - target).abs() < 0.001 && state.paused
        });
        self.out.pause(false);
        self.hear(sample_count, false).1
    }
}

fn until(seconds: u64, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !check() {
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for player state"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

fn freq(samples: &[f32], expected: f64) {
    let tail = &samples[samples.len() / 5..];
    let cycles = tail
        .windows(2)
        .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
        .count();
    let measured = cycles as f64 * f64::from(RATE) / tail.len() as f64;
    assert!(
        (measured - expected).abs() < 14.0,
        "Expected {expected} Hz, measured {measured} Hz"
    );
}

fn exact_flac(samples: &[f32], offset: f64) -> f32 {
    let start = (offset * f64::from(RATE)).round() as usize;
    samples
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            let i = start + index;
            let expected = (11000.0
                * (2.0 * std::f64::consts::PI * (220 + (i / RATE as usize) * 45) as f64 * i as f64
                    / f64::from(RATE))
                .sin())
            .round() as f32
                / 32768.0
                * 0.8;
            (expected - sample).abs()
        })
        .fold(0.0, f32::max)
}

fn verify(checks: &mut Checks) -> Value {
    checks.play("range.flac", None, None, "flac", Quality::Original, false);
    let (_, initial) = checks.hear(12_000, false);
    let forward = checks.seek(12.25, 8_000);
    let backward = checks.seek(2.25, 8_000);
    let error = exact_flac(&initial, 0.0)
        .max(exact_flac(&forward, 12.25))
        .max(exact_flac(&backward, 2.25));
    assert!(error < 0.00005, "FLAC seek samples differ by {error}");

    checks.play("chunked-slow.mp3", None, None, "mp3", Quality::K128, false);
    let (_, initial) = checks.hear(9_600, false);
    freq(&initial, 220.0);
    checks.player.seek(8.25);
    until(3, || {
        checks
            .out
            .snapshot()
            .note
            .as_deref()
            .is_some_and(|note| note.contains("not yet seekable"))
    });
    thread::sleep(Duration::from_millis(2200));
    let mp3 = checks.seek(8.25, 9600);
    freq(&mp3, 580.0);

    checks.play("drop.flac", None, None, "flac", Quality::Original, false);
    let mut continuous = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(16);
    let mut recovered;
    let mut recovery_frame = None;
    loop {
        recovered = checks.tick(deadline, true, &mut continuous);
        assert!(
            !matches!(recovered.phase, Phase::Failed(_)),
            "{recovered:?}"
        );
        if recovered.recoveries == 1 {
            let boundary = *recovery_frame.get_or_insert(continuous.len());
            if recovered.phase == Phase::Playing && continuous.len() >= boundary + 4_800 {
                break;
            }
        }
    }
    assert!(
        recovery_frame.unwrap() >= 4_800,
        "The interruption occurred before audible playback"
    );
    let recovery_error = exact_flac(&continuous, 0.0);
    assert!(
        recovery_error < 0.00005,
        "Resumed FLAC samples differ by {recovery_error}"
    );
    assert_eq!(recovered.recoveries, 1);

    checks.play("once.mp3", None, None, "mp3", Quality::K128, true);
    until(3, || checks.out.snapshot().recoveries == 1);
    assert_eq!(checks.out.snapshot().position, 0.0);
    assert!(checks.out.snapshot().paused);
    assert_eq!(checks.out.render_fixture(&mut [[0.0; 2]; BLOCK]), 0);
    checks.out.pause(false);
    checks.hear(4800, false);

    for (file, count) in [("always.mp3", 1), ("missing.mp3", 0)] {
        checks.play(file, None, None, "mp3", Quality::K128, false);
        until(4, || {
            matches!(checks.out.snapshot().phase, Phase::Failed(_))
        });
        assert_eq!(checks.out.snapshot().recoveries, count);
    }

    checks.play(
        "fallback.mp3",
        Some("tone.m3u8"),
        None,
        "mp3",
        Quality::K192,
        false,
    );
    let (hls, samples) = checks.hear(9600, false);
    assert_eq!(hls.route, Some(Route::Hls(Quality::K192)), "{hls:?}");
    freq(&samples, 220.0);
    let hls_seek = checks.seek(8.25, 9600);
    freq(&hls_seek, 580.0);

    checks.play(
        "fallback.mp3",
        Some("event.m3u8"),
        None,
        "mp3",
        Quality::K192,
        false,
    );
    let (hls, _) = checks.hear(9600, false);
    assert_eq!(hls.route, Some(Route::Hls(Quality::K192)), "{hls:?}");

    checks.play(
        "fallback.mp3",
        Some("missing.m3u8"),
        None,
        "mp3",
        Quality::K192,
        false,
    );
    let (hls, _) = checks.hear(9600, false);
    assert_eq!(hls.route, Some(Route::Direct(Quality::K192)));
    assert_eq!(hls.recoveries, 1);

    checks.play(
        "fallback.mp3",
        Some("foreign.m3u8"),
        None,
        "mp3",
        Quality::K192,
        false,
    );
    until(3, || {
        matches!(checks.out.snapshot().phase, Phase::Failed(_))
    });
    assert_eq!(checks.out.snapshot().recoveries, 0);

    checks.play(
        "unsupported.orig",
        None,
        Some("fallback.mp3"),
        "flac",
        Quality::Original,
        false,
    );
    let (original, _) = checks.hear(9600, false);
    assert_eq!(original.route, Some(Route::Direct(Quality::K128)));
    assert_eq!(original.recoveries, 1);

    checks.play("slow.mp3", None, None, "mp3", Quality::K128, false);
    thread::sleep(Duration::from_millis(80));
    let start = Instant::now();
    checks.play("range.flac", None, None, "flac", Quality::Original, false);
    let (_, pcm) = checks.hear(4800, false);
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "A replaced HTTP request delayed the new stream"
    );
    assert!(exact_flac(&pcm, 0.0) < 0.00005);

    checks.play(
        "resample.flac",
        None,
        None,
        "flac",
        Quality::Original,
        false,
    );
    let (_, pcm) = checks.hear(9_600, false);
    freq(&pcm, 220.0);
    checks.seek(5.25, 9600);
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut remaining = Vec::new();
    let resampled_duration = loop {
        let state = checks.tick(deadline, false, &mut remaining);
        assert!(!matches!(state.phase, Phase::Failed(_)), "{state:?}");
        if state.phase == Phase::Ended {
            break state.position;
        }
    };
    assert!(
        (resampled_duration - 16.0).abs() < 0.002,
        "Incorrect resampled duration: {resampled_duration}"
    );

    json!({
        "range_flac_seek_seconds": [12.25, 2.25],
        "range_seek_max_abs_sample_error": error,
        "chunked_mp3_seek_seconds": 8.25,
        "chunked_mp3_early_seek_rejected": true,
        "flac_recovery_max_abs_sample_error": recovery_error,
        "flac_samples_before_recovery": recovery_frame.unwrap(),
        "44100_to_48000_hz_resampled_track_end_seconds": resampled_duration,
        "paused_play_preserved": true,
        "http_503_retried_once": true,
        "http_404_not_retried": true,
        "hls_seek_seconds": 8.25,
        "hls_event_started": true,
        "hls_same_bitrate_fallback": true,
        "hls_foreign_segment_rejected_without_fallback": true,
        "original_unsupported_fallback": true,
        "replaced_http_request_cancelled": true,
    })
}

fn gapless(checks: &mut Checks) -> Value {
    let request = |path| {
        MediaRequest::fixture(
            checks.http.clone(),
            checks.base.join(path).unwrap(),
            None,
            None,
            "flac",
            Quality::Original,
        )
        .unwrap()
    };
    let requests = (
        request("joined.flac"),
        request("resample.flac"),
        request("resample.flac"),
        request("range.flac"),
        request("range.flac"),
    );
    checks
        .player
        .play_track(10, requests.0, 0.0, Some(32.0), false);
    let (last, joined) = collect_track(checks, Vec::new());
    assert!(last.position > 31.99);

    checks
        .player
        .play_track(11, requests.1, 0.0, Some(16.0), false);
    let (_, start) = checks.hear(4800, false);
    checks.player.plan_successor(Some(NextTrack {
        from: 11,
        key: 12,
        request: requests.2,
        duration: Some(16.0),
    }));
    let (last, continuous) = collect_track(checks, start);
    assert_eq!(last.track_key, 12);
    assert!(last.gapless_transition);
    assert_eq!(joined.len(), continuous.len());
    let error = joined
        .iter()
        .zip(&continuous)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(
        error < 0.000001,
        "Resampled album differs from one continuous file by {error}"
    );

    checks
        .player
        .play_track(13, requests.3, 0.0, Some(16.0), false);
    let (_, start) = checks.hear(4800, false);
    checks.player.plan_successor(Some(NextTrack {
        from: 13,
        key: 14,
        request: requests.4,
        duration: Some(16.0),
    }));
    let (last, pcm) = collect_track(checks, start);
    assert_eq!(last.track_key, 14);
    let (first, second) = pcm.split_at(16 * RATE as usize);
    assert_eq!(first.len(), second.len());
    assert_eq!(first, second);
    assert_eq!(exact_flac(first, 0.0), 0.0);
    json!({"same_rate_frames": pcm.len(), "continuous_resample_frames": joined.len(), "continuous_resample_max_abs_error": error})
}

fn collect_track(checks: &Checks, mut pcm: Vec<f32>) -> (Snapshot, Vec<f32>) {
    let deadline = Instant::now() + Duration::from_secs(22);
    loop {
        let state = checks.tick(deadline, false, &mut pcm);
        assert!(!matches!(state.phase, Phase::Failed(_)), "{state:?}");
        thread::sleep(Duration::from_millis(1));
        if state.phase == Phase::Ended {
            return (state, pcm);
        }
    }
}

fn rss_kib() -> u64 {
    let out = Command::new("/bin/ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

fn simulated_soak(checks: &mut Checks, hours: f64) -> Value {
    let tracks = (hours * 3600.0 / 16.0).ceil() as usize;
    let baseline = rss_kib();
    let mut peak = baseline;
    let mut media_seconds = 0.0;
    for index in 0..tracks {
        let (path, extension, quality) = if index % 2 == 0 {
            ("soak.flac", "flac", Quality::Original)
        } else {
            ("chunked-soak.mp3", "mp3", Quality::K128)
        };
        checks.play(path, None, None, extension, quality, false);
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut discard = Vec::with_capacity(BLOCK);
        loop {
            discard.clear();
            let state = checks.tick(deadline, false, &mut discard);
            match state.phase {
                Phase::Ended => {
                    media_seconds += state.position;
                    break;
                }
                Phase::Failed(_) => panic!("Soak failed: {state:?}"),
                _ => {}
            }
        }
        if (index + 1).is_multiple_of(5) {
            peak = peak.max(rss_kib());
        }
    }
    json!({"tracks": tracks, "audio_seconds": media_seconds, "start_rss_kib": baseline, "peak_sampled_rss_kib": peak, "end_rss_kib": rss_kib()})
}

fn device_measurement(base: Url, soak_seconds: u64) -> Result<Value, Box<dyn Error>> {
    let (out, _stream) = Output::device()?;
    out.adjust_volume(-1.0);
    let mut checks = Checks::new(base, out)?;
    let idle = rss_kib();
    let mut trials = Vec::new();
    for (name, ext, quality, hls) in [
        ("latency-mp3", "mp3", Quality::K128, None),
        ("latency-original", "flac", Quality::Original, None),
        (
            "latency-hls",
            "mp3",
            Quality::K192,
            Some("latency-hls.m3u8"),
        ),
    ] {
        let mut times = Vec::new();
        for _ in 0..9 {
            let start = Instant::now();
            checks.play(&format!("{name}.{ext}"), hls, None, ext, quality, false);
            until(10, || {
                let state = checks.out.snapshot();
                assert!(!matches!(state.phase, Phase::Failed(_)), "{state:?}");
                state.first_audio.is_some()
            });
            let first = checks.out.snapshot().first_audio.unwrap();
            times.push(first.duration_since(start).as_secs_f64() * 1000.0);
            checks.player.stop();
            thread::sleep(Duration::from_millis(35));
        }
        let first = times[0];
        times.sort_by(f64::total_cmp);
        trials.push(
            json!({"route": name, "trials": times.len(), "first_ms": first,
            "median_ms": times[times.len() / 2], "max_ms": times[times.len() - 1]}),
        );
    }
    let mut peak = rss_kib();
    let start = Instant::now();
    let mut picks = 0;
    while start.elapsed() < Duration::from_secs(soak_seconds) {
        let (file, ext, quality) = if picks % 2 == 0 {
            ("soak.flac", "flac", Quality::Original)
        } else {
            ("chunked-soak.mp3", "mp3", Quality::K128)
        };
        checks.play(file, None, None, ext, quality, false);
        let track_deadline = Instant::now() + Duration::from_secs(22);
        loop {
            let state = checks.out.snapshot();
            assert!(
                Instant::now() < track_deadline && !matches!(state.phase, Phase::Failed(_)),
                "CoreAudio soak: {state:?}"
            );
            if state.phase == Phase::Ended || start.elapsed() >= Duration::from_secs(soak_seconds) {
                break;
            }
            peak = peak.max(rss_kib());
            thread::sleep(Duration::from_secs(1));
        }
        picks += 1;
    }
    let end = rss_kib();
    Ok(
        json!({"output_rate_hz": checks.out.rate, "start_latency": trials, "idle_rss_kib": idle,
        "soak_wall_seconds": start.elapsed().as_secs_f64(), "peak_rss_kib": peak, "end_rss_kib": end, "tracks": picks}),
    )
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    let base = Url::parse(&args[1])?;
    let seconds: u64 = args[2].parse()?;
    let hours: f64 = args[3].parse()?;
    let device = if args.iter().any(|arg| arg == "--skip-device") {
        Value::Null
    } else {
        match device_measurement(base.clone(), seconds) {
            Ok(data) => data,
            Err(error) => json!({"error": error.to_string()}),
        }
    };
    let mut checks = Checks::new(base, Output::new(RATE))?;
    let outcomes = verify(&mut checks);
    let transitions = gapless(&mut checks);
    let soak = simulated_soak(&mut checks, hours);
    println!(
        "{}",
        json!({"checks": outcomes, "gapless": transitions, "core_audio": device, "accelerated_soak": soak})
    );
    Ok(())
}
