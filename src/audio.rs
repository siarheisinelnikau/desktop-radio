use crate::{
    hls,
    http::Response,
    model::{FREQUENCIES, Preferences, Station},
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    io::{self, Cursor, Read, Seek, SeekFrom},
    mem::size_of,
    path::Path,
    ptr::null_mut,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use symphonia::{
    core::{
        audio::SampleBuffer,
        codecs::{CODEC_TYPE_NULL, DecoderOptions},
        errors::Error as DecodeError,
        formats::FormatOptions,
        io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions},
        meta::MetadataOptions,
        probe::Hint,
    },
    default::{get_codecs, get_probe},
};
use url::Url;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    Media::Audio::*,
    System::Threading::{CreateEventW, WaitForSingleObject},
};

pub const METER_SIZE: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Ready,
    Connecting,
    Playing(Option<String>),
    Unavailable,
    AudioDevice,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Mix {
    volume: u8,
    enabled: bool,
    gains: [i8; 10],
}

impl From<&Preferences> for Mix {
    fn from(prefs: &Preferences) -> Self {
        Self {
            volume: prefs.volume,
            enabled: prefs.equalizer_enabled,
            gains: prefs.equalizer,
        }
    }
}

pub struct Signal {
    pub values: [f32; METER_SIZE],
    pub rate: u32,
    pub revision: u64,
}

impl Default for Signal {
    fn default() -> Self {
        Self {
            values: [0.; METER_SIZE],
            rate: 48_000,
            revision: 0,
        }
    }
}

pub struct Engine {
    shared: Arc<Shared>,
    requests: mpsc::Sender<(u64, Station)>,
}

struct Shared {
    revision: AtomicU64,
    meter_enabled: AtomicBool,
    signal: Mutex<Signal>,
    mix: Mutex<Mix>,
}

impl Engine {
    pub fn start(prefs: &Preferences, notify: impl Fn(u64, Status) + Send + 'static) -> Self {
        let (requests, receiver) = mpsc::channel::<(u64, Station)>();
        let shared = Arc::new(Shared {
            revision: AtomicU64::new(0),
            meter_enabled: AtomicBool::new(false),
            signal: Mutex::new(Signal::default()),
            mix: Mutex::new(Mix::from(prefs)),
        });
        let worker = shared.clone();
        thread::spawn(move || {
            while let Ok((revision, station)) = receiver.recv() {
                let cancel = Cancel(&worker, revision);
                if cancel.stopped() {
                    continue;
                }
                let mut output = Audio::new(&worker, cancel, station.track_source_id, &notify);
                let mut failure = Status::Unavailable;
                for stream in &station.streams {
                    if cancel.stopped() {
                        break;
                    }
                    output.status(Status::Connecting);
                    let result = play_url(stream, &mut output);
                    let _ = output.drain();
                    if cancel.stopped() {
                        break;
                    }
                    if output.device_failed {
                        failure = Status::AudioDevice;
                        break;
                    }
                    if result.is_ok() && stream.starts_with("file:") {
                        break;
                    }
                }
                if !cancel.stopped() {
                    output.status(failure);
                }
            }
        });
        Self { shared, requests }
    }

    pub fn play(&self, station: Station) -> u64 {
        let revision = self.stop();
        let _ = self.requests.send((revision, station));
        revision
    }

    pub fn stop(&self) -> u64 {
        self.shared.revision.fetch_add(1, Relaxed) + 1
    }

    pub fn mix(&self, prefs: &Preferences) {
        *self.shared.mix.lock().unwrap() = Mix::from(prefs);
    }

    pub fn watch(&self, enabled: bool) {
        self.shared.meter_enabled.store(enabled, Relaxed);
    }

    pub fn signal(&self) -> Signal {
        let signal = self.shared.signal.lock().unwrap();
        Signal {
            values: signal.values,
            rate: signal.rate,
            revision: signal.revision,
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone, Copy)]
struct Cancel<'a>(&'a Shared, u64);

impl Cancel<'_> {
    fn stopped(self) -> bool {
        self.0.revision.load(Relaxed) != self.1
    }
    fn check(self) -> Result<()> {
        if self.stopped() {
            bail!("Playback cancelled");
        }
        Ok(())
    }
}

pub fn diagnose(path: &Path, report: &Path) -> Result<()> {
    let shared = Shared {
        revision: AtomicU64::new(0),
        meter_enabled: AtomicBool::new(true),
        signal: Mutex::new(Signal::default()),
        mix: Mutex::new(Mix {
            volume: 0,
            enabled: true,
            gains: [0, 0, 0, 0, 0, 6, 0, 0, 0, 0],
        }),
    };
    let events = Mutex::new(Vec::new());
    let notify = |_, status: Status| events.lock().unwrap().push(format!("{status:?}"));
    let mut output = Audio::new(&shared, Cancel(&shared, 0), None, &notify);
    output.max_frames = Some(96_000);
    let source = path.to_str().context("Invalid test source path")?;
    let url = if source.starts_with("http:") || source.starts_with("https:") {
        source.to_owned()
    } else {
        Url::from_file_path(path.canonicalize()?)
            .map_err(|_| anyhow::anyhow!("Invalid audio test file path"))?
            .to_string()
    };
    let decoded = play_url(&url, &mut output);
    if decoded.is_ok() {
        output.drain()?;
    }
    let mut filter = Equalizer::new(48_000);
    filter.configure([0, 0, 0, 0, 0, 6, 0, 0, 0, 0]);
    let mut peak = 0_f32;
    for i in 0..48_000 {
        let signal = (std::f32::consts::TAU * 1000. * i as f32 / 48_000.).sin();
        let mut samples = [signal, signal];
        filter.process(&mut samples);
        if i > 12_000 {
            peak = peak.max(samples[0].abs());
        }
    }
    std::fs::write(
        report,
        serde_json::to_vec_pretty(&serde_json::json!({
            "events": *events.lock().unwrap(), "frames": output.frames, "gain_1_khz": peak,
            "error": decoded.as_ref().err().map(|e| format!("{e:#}")),
        }))?,
    )?;
    decoded?;
    if output.frames == 0 || !(1.9..2.1).contains(&peak) {
        bail!("Audio self-test failed");
    }
    Ok(())
}

fn play_url(url: &str, output: &mut Audio<'_>) -> Result<()> {
    let url = Url::parse(url)?;
    if url.scheme() == "file" {
        let path = url
            .to_file_path()
            .map_err(|_| anyhow::anyhow!("Invalid file URL"))?;
        return decode(
            Box::new(std::fs::File::open(&path)?),
            extension(&url),
            output,
        );
    }
    let response = Response::open(&url, "Icy-MetaData: 1\r\n", 5000)?;
    let base = response.effective_url(&url);
    let kind = response
        .header("Content-Type")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let is_hls = extension(&base) == "m3u8" || kind.contains("mpegurl");
    if is_hls || extension(&base) == "pls" || kind.contains("scpls") {
        let text = String::from_utf8(response.collect(768 * 1024)?)?;
        if text.trim_start_matches('\u{feff}').starts_with("#EXTM3U") && text.contains("#EXT-X-") {
            return play_hls(base, text, output);
        }
        let next = text
            .lines()
            .map(str::trim)
            .filter_map(|line| {
                if let Some((key, value)) = line.split_once('=') {
                    if key.to_ascii_lowercase().starts_with("file") {
                        return Some(value);
                    }
                }
                if line.starts_with("http://") || line.starts_with("https://") {
                    return Some(line);
                }
                None
            })
            .next()
            .context("No audio stream in playlist")?;
        let next = base.join(next)?;
        let response = Response::open(&next, "Icy-MetaData: 1\r\n", 5000)?;
        let next = response.effective_url(&next);
        return decode_response(response, &next, output);
    }
    decode_response(response, &base, output)
}

fn decode_response(response: Response, url: &Url, output: &mut Audio<'_>) -> Result<()> {
    let mime = response.header("Content-Type").unwrap_or_default();
    let interval = response
        .header("icy-metaint")
        .and_then(|s| s.parse().ok())
        .filter(|n| (1..=1_000_000).contains(n));
    let reader = Icy {
        response,
        interval,
        remaining: interval.unwrap_or_default(),
        metadata: output.stream_track.clone(),
    };
    let kind = match mime.split(';').next().map(str::trim).unwrap_or("") {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/aac" | "audio/aacp" | "audio/x-aac" => "aac",
        "audio/ogg" | "application/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/mp4" => "m4a",
        _ => extension(url),
    };
    decode(Box::new(reader), kind, output)
}

fn play_hls(mut base: Url, mut text: String, output: &mut Audio<'_>) -> Result<()> {
    let mut remaining_hops = 4;
    let mut next_sequence = None;
    let mut saved_init: Option<(String, Vec<u8>)> = None;
    let mut last_segment = Instant::now();
    loop {
        output.cancel.check()?;
        let list = hls::parse(&base, &text)?;
        if let Some(variant) = list.variant {
            if remaining_hops == 0 {
                bail!("Too many HLS variants");
            }
            remaining_hops -= 1;
            let response = Response::open(&variant, "", 5000)?;
            base = response.effective_url(&variant);
            text = String::from_utf8(response.collect(768 * 1024)?)?;
            continue;
        }
        let start = next_sequence
            .unwrap_or_else(|| {
                list.sequence
                    + if list.ended {
                        0
                    } else {
                        list.segments.len().saturating_sub(2) as u64
                    }
            })
            .max(list.sequence);
        let first = (start - list.sequence) as usize;
        for (offset, part) in list.segments.iter().enumerate().skip(first) {
            output.cancel.check()?;
            let bytes = fetch_part(part)?;
            let (audio, format) = if let Some(init) = &part.initialization {
                let key = format!("{}:{}", init.url, init.headers());
                if saved_init.as_ref().is_none_or(|(saved, _)| saved != &key) {
                    saved_init = Some((key, fetch_part(init)?));
                }
                let mut initialized = saved_init.as_ref().unwrap().1.clone();
                initialized.extend(bytes);
                (initialized, "m4a")
            } else if extension(&part.url) == "ts" || bytes.starts_with(&[0x47]) {
                hls::transport_audio(&bytes)?
            } else {
                (bytes, extension(&part.url))
            };
            decode(Box::new(Cursor::new(audio)), format, output)?;
            if output.reached_limit() {
                return Ok(());
            }
            next_sequence = Some(list.sequence + offset as u64 + 1);
            last_segment = Instant::now();
        }
        if list.ended {
            return Ok(());
        }
        if last_segment.elapsed() > Duration::from_secs(25) {
            bail!("HLS playlist stopped advancing");
        }
        if next_sequence == Some(start) || first >= list.segments.len() {
            let until = Instant::now() + list.wait;
            while Instant::now() < until {
                output.cancel.check()?;
                thread::sleep(Duration::from_millis(100));
            }
        }
        let response = Response::open(&base, "", 5000)?;
        text = String::from_utf8(response.collect(768 * 1024)?)?;
    }
}

fn fetch_part(part: &hls::Part) -> Result<Vec<u8>> {
    Response::open(&part.url, &part.headers(), 5000)?.collect(16 * 1024 * 1024)
}

fn extension(url: &Url) -> &str {
    let filename = url.path().rsplit('/').next().unwrap_or_default();
    filename
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension)
}

struct Icy {
    response: Response,
    interval: Option<usize>,
    remaining: usize,
    metadata: Arc<Mutex<Option<String>>>,
}

impl Read for Icy {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        let Some(interval) = self.interval else {
            return self.response.read(target);
        };
        if target.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut length = [0];
            if self.response.read(&mut length)? == 0 {
                return Ok(0);
            }
            if length[0] != 0 {
                let mut metadata = vec![0; length[0] as usize * 16];
                self.response.read_exact(&mut metadata)?;
                let text = String::from_utf8_lossy(&metadata);
                if let Some((_, title)) = text.split_once("StreamTitle='") {
                    if let Some((title, _)) = title.split_once("';") {
                        let title: String = title
                            .trim()
                            .chars()
                            .filter(|c| !c.is_control())
                            .take(300)
                            .collect();
                        *self.metadata.lock().unwrap() = (!title.is_empty()).then_some(title);
                    }
                }
            }
            self.remaining = interval;
        }
        let length = target.len().min(self.remaining);
        let count = self.response.read(&mut target[..length])?;
        self.remaining -= count;
        Ok(count)
    }
}

impl Seek for Icy {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Live audio is not seekable",
        ))
    }
}

impl MediaSource for Icy {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

fn decode(input: Box<dyn MediaSource>, extension: &str, output: &mut Audio<'_>) -> Result<()> {
    let mut hint = Hint::new();
    if !extension.is_empty() {
        hint.with_extension(extension);
    }
    let stream = MediaSourceStream::new(input, MediaSourceStreamOptions::default());
    let mut format = get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )?
        .format;
    let track = format
        .tracks()
        .iter()
        .find(|track| {
            track.codec_params.codec != CODEC_TYPE_NULL && track.codec_params.sample_rate.is_some()
        })
        .or_else(|| format.default_track())
        .context("No audio track")?;
    let track_id = track.id;
    let mut decoder = get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
    let mut scratch = None::<SampleBuffer<f32>>;
    let mut errors = 0;
    let start = output.frames;
    loop {
        output.cancel.check()?;
        if output.reached_limit() {
            break;
        }
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(DecodeError::IoError(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(error) => return Err(error.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(frame) => {
                errors = 0;
                let spec = *frame.spec();
                let capacity = frame.capacity();
                if scratch
                    .as_ref()
                    .is_none_or(|scratch| scratch.capacity() < capacity * spec.channels.count())
                {
                    scratch = Some(SampleBuffer::new(capacity as u64, spec));
                }
                let scratch = scratch.as_mut().unwrap();
                scratch.copy_interleaved_ref(frame);
                output.write(scratch.samples(), spec.rate, spec.channels.count())?;
            }
            Err(DecodeError::DecodeError(_)) if errors < 12 => errors += 1,
            Err(error) => return Err(error.into()),
        }
    }
    if output.frames == start {
        bail!("Stream contained no decodable audio");
    }
    Ok(())
}

struct Audio<'a> {
    shared: &'a Shared,
    cancel: Cancel<'a>,
    notify: &'a dyn Fn(u64, Status),
    last_status: Option<Status>,
    wave: Option<Wave>,
    equalizer: Equalizer,
    mix: Option<Mix>,
    stream_track: Arc<Mutex<Option<String>>>,
    feed: Option<Feed>,
    mono: [f32; METER_SIZE],
    mono_head: usize,
    stereo: Vec<i16>,
    device_failed: bool,
    frames: u64,
    max_frames: Option<u64>,
}

impl<'a> Audio<'a> {
    fn new(
        shared: &'a Shared,
        cancel: Cancel<'a>,
        feed_id: Option<u32>,
        notify: &'a dyn Fn(u64, Status),
    ) -> Self {
        Self {
            shared,
            cancel,
            notify,
            last_status: None,
            wave: None,
            equalizer: Equalizer::new(48_000),
            mix: None,
            stream_track: Arc::new(Mutex::new(None)),
            feed: feed_id.map(Feed::new),
            mono: [0.; METER_SIZE],
            mono_head: 0,
            stereo: Vec::new(),
            device_failed: false,
            frames: 0,
            max_frames: None,
        }
    }

    fn status(&mut self, next: Status) {
        if self.last_status.as_ref() != Some(&next) && !self.cancel.stopped() {
            (self.notify)(self.cancel.1, next.clone());
            self.last_status = Some(next);
        }
    }

    fn reached_limit(&self) -> bool {
        self.max_frames.is_some_and(|max| self.frames >= max)
    }

    fn write(&mut self, input: &[f32], rate: u32, channels: usize) -> Result<()> {
        if input.is_empty() || channels == 0 {
            return Ok(());
        }
        self.cancel.check()?;
        if !(8_000..=192_000).contains(&rate) {
            bail!("Unsupported sample rate");
        }
        if self.wave.as_ref().is_none_or(|wave| wave.rate != rate) {
            self.drain()?;
            self.wave = None;
            self.wave = Some(Wave::open(rate).inspect_err(|_| self.device_failed = true)?);
            self.equalizer = Equalizer::new(rate);
            self.mix = None;
        }
        let mix = *self.shared.mix.lock().unwrap();
        if self
            .mix
            .is_none_or(|old| old.enabled != mix.enabled || old.gains != mix.gains)
        {
            self.equalizer
                .configure(if mix.enabled { mix.gains } else { [0; 10] });
        }
        self.mix = Some(mix);
        self.stereo.clear();
        let actually_watch = self.shared.meter_enabled.load(Relaxed);
        let volume = mix.volume as f32 / 100.;
        for frame in input.chunks_exact(channels) {
            let mut stereo = [frame[0], *frame.get(1).unwrap_or(&frame[0])];
            self.equalizer.process(&mut stereo);
            if actually_watch {
                self.mono[self.mono_head] = (stereo[0] + stereo[1]) * 0.5;
                self.mono_head = (self.mono_head + 1) % METER_SIZE;
            }
            self.stereo.extend(
                stereo.map(|value| (value * volume * 32767.).clamp(-32768., 32767.) as i16),
            );
        }
        self.wave
            .as_mut()
            .unwrap()
            .submit(&self.stereo, self.cancel)
            .inspect_err(|_| self.device_failed = !self.cancel.stopped())?;
        self.frames += (self.stereo.len() / 2) as u64;
        if actually_watch {
            let mut signal = self.shared.signal.lock().unwrap();
            let (tail, head) = signal.values.split_at_mut(METER_SIZE - self.mono_head);
            tail.copy_from_slice(&self.mono[self.mono_head..]);
            head.copy_from_slice(&self.mono[..self.mono_head]);
            signal.rate = rate;
            signal.revision += 1;
        }
        let track = self.stream_track.lock().unwrap().clone();
        let track = track.or_else(|| self.feed.as_mut().and_then(Feed::track));
        self.status(Status::Playing(track));
        Ok(())
    }

    fn drain(&mut self) -> Result<()> {
        if let Some(wave) = &self.wave {
            wave.drain(self.cancel)?;
        }
        Ok(())
    }
}

struct Feed {
    id: u32,
    next: Instant,
    title: Option<String>,
    result: Option<mpsc::Receiver<Option<String>>>,
}

impl Feed {
    fn new(id: u32) -> Self {
        Self {
            id,
            next: Instant::now(),
            title: None,
            result: None,
        }
    }

    fn track(&mut self) -> Option<String> {
        if let Some(result) = &self.result {
            match result.try_recv() {
                Ok(title) => {
                    self.title = title;
                    self.result = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.result = None;
                }
                Err(mpsc::TryRecvError::Empty) => (),
            }
        }
        if self.result.is_none() && self.next <= Instant::now() {
            let (sender, result) = mpsc::channel();
            let id = self.id;
            self.next = Instant::now() + Duration::from_secs(18);
            self.result = Some(result);
            thread::spawn(move || {
                let _ = sender.send(feed_track(id).ok().flatten());
            });
        }
        self.title.clone()
    }
}

fn feed_track(id: u32) -> Result<Option<String>> {
    let url = Url::parse(&format!("https://radioplayer.by/api/songs/current/{id}"))?;
    let json: Value = serde_json::from_slice(
        &Response::open(&url, "Accept: application/json\r\n", 3500)?.collect(16 * 1024)?,
    )?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let start = json["lastStartPlaying"]
        .as_str()
        .and_then(|raw| {
            time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339).ok()
        })
        .map_or(0, |date| date.unix_timestamp());
    let duration = json["duration"].as_i64().unwrap_or(0);
    if json["hasData"].as_bool() != Some(true)
        || json["id"].as_u64().unwrap_or(0) == 0
        || !(1..=21_600).contains(&duration)
        || start > now + 30
        || start + duration + 25 < now
    {
        return Ok(None);
    }
    let title = json["name"].as_str().unwrap_or("").trim();
    let artist = json["artist"].as_str().unwrap_or("").trim();
    if title.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        if artist.is_empty() {
            title.to_owned()
        } else {
            format!("{artist} — {title}")
        }
        .chars()
        .filter(|c| !c.is_control())
        .take(300)
        .collect(),
    ))
}

struct Equalizer {
    rate: u32,
    filters: Vec<Filter>,
}

impl Equalizer {
    fn new(rate: u32) -> Self {
        Self {
            rate,
            filters: Vec::with_capacity(10),
        }
    }

    fn configure(&mut self, gains: [i8; 10]) {
        self.filters.clear();
        for (frequency, gain) in FREQUENCIES.into_iter().zip(gains) {
            if gain == 0 || frequency * 2 >= self.rate {
                continue;
            }
            let angle = std::f64::consts::TAU * frequency as f64 / self.rate as f64;
            let amplitude = 10_f64.powf(gain as f64 / 40.);
            let alpha = angle.sin() / (2. * 1.41);
            let denominator = 1. + alpha / amplitude;
            self.filters.push(Filter {
                b0: ((1. + alpha * amplitude) / denominator) as f32,
                b1: (-2. * angle.cos() / denominator) as f32,
                b2: ((1. - alpha * amplitude) / denominator) as f32,
                a2: ((1. - alpha / amplitude) / denominator) as f32,
                state: [[0.; 2]; 2],
            });
        }
    }

    fn process(&mut self, stereo: &mut [f32; 2]) {
        for filter in &mut self.filters {
            for (sample, [first, second]) in stereo.iter_mut().zip(&mut filter.state) {
                let next = filter.b0 * *sample + *first;
                *first = filter.b1 * (*sample - next) + *second;
                *second = filter.b2 * *sample - filter.a2 * next;
                *sample = next;
            }
        }
    }
}

struct Filter {
    b0: f32,
    b1: f32,
    b2: f32,
    a2: f32,
    state: [[f32; 2]; 2],
}

struct Wave {
    device: HWAVEOUT,
    event: HANDLE,
    buffers: Vec<Box<WaveBuffer>>,
    next: usize,
    rate: u32,
}

struct WaveBuffer {
    header: WAVEHDR,
    samples: Vec<i16>,
}
const HEADER_SIZE: u32 = size_of::<WAVEHDR>() as u32;

impl Wave {
    fn open(rate: u32) -> Result<Self> {
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        let mut wave = Self {
            device: null_mut(),
            event,
            buffers: Vec::new(),
            next: 0,
            rate,
        };
        let format = WAVEFORMATEX {
            wFormatTag: 1,
            nChannels: 2,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * 4,
            nBlockAlign: 4,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        check_audio(unsafe {
            waveOutOpen(
                &mut wave.device,
                WAVE_MAPPER,
                &format,
                event as usize,
                0,
                CALLBACK_EVENT,
            )
        })?;
        let capacity = (rate as usize / 10) * 2;
        for _ in 0..8 {
            let mut buffer = Box::new(WaveBuffer {
                header: WAVEHDR::default(),
                samples: vec![0; capacity],
            });
            buffer.header.lpData = buffer.samples.as_mut_ptr().cast();
            buffer.header.dwBufferLength = (capacity * size_of::<i16>()) as u32;
            check_audio(unsafe {
                waveOutPrepareHeader(wave.device, &mut buffer.header, HEADER_SIZE)
            })?;
            wave.buffers.push(buffer);
        }
        Ok(wave)
    }

    fn submit(&mut self, mut pcm: &[i16], cancel: Cancel<'_>) -> Result<()> {
        while !pcm.is_empty() {
            let buffer = &mut self.buffers[self.next];
            Self::wait(self.event, &buffer.header, cancel)?;
            let count = pcm.len().min(buffer.samples.len());
            buffer.samples[..count].copy_from_slice(&pcm[..count]);
            buffer.header.dwBufferLength = (count * size_of::<i16>()) as u32;
            check_audio(unsafe { waveOutWrite(self.device, &mut buffer.header, HEADER_SIZE) })?;
            self.next = (self.next + 1) % self.buffers.len();
            pcm = &pcm[count..];
        }
        Ok(())
    }

    fn drain(&self, cancel: Cancel<'_>) -> Result<()> {
        for buffer in &self.buffers {
            Self::wait(self.event, &buffer.header, cancel)?;
        }
        Ok(())
    }

    fn wait(event: HANDLE, header: *const WAVEHDR, cancel: Cancel<'_>) -> Result<()> {
        let mut start = None;
        while unsafe { (&raw const (*header).dwFlags).read_volatile() & WHDR_INQUEUE != 0 } {
            cancel.check()?;
            if start.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(2) {
                bail!("Windows audio output did not release a buffer");
            }
            unsafe {
                WaitForSingleObject(event, 50);
            }
        }
        Ok(())
    }
}

impl Drop for Wave {
    fn drop(&mut self) {
        unsafe {
            if !self.device.is_null() {
                waveOutReset(self.device);
                for buffer in &mut self.buffers {
                    waveOutUnprepareHeader(self.device, &mut buffer.header, HEADER_SIZE);
                }
                waveOutClose(self.device);
            }
            CloseHandle(self.event);
        }
    }
}

fn check_audio(error: u32) -> Result<()> {
    if error != 0 {
        bail!("Windows audio output error {error}");
    }
    Ok(())
}
