use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::audio_device::preferred_input_config;
use crate::legacy_core::config::Config;
use crate::legacy_core::config::find_codex_home;
use base64::Engine;
use codex_app_server_protocol::AuthMode;
use codex_client::build_reqwest_client_with_custom_ca;
use codex_config::types::AuthCredentialsStoreMode;
use codex_login::CodexAuth;
use codex_login::default_client::get_codex_user_agent;
use codex_protocol::protocol::ConversationAudioParams;
use codex_protocol::protocol::RealtimeAudioFrame;
use cpal::traits::DeviceTrait;
use cpal::traits::HostTrait;
use cpal::traits::StreamTrait;
use hound::SampleFormat;
use hound::WavSpec;
use hound::WavWriter;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::io::Cursor;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU16;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;
use tracing::error;
use tracing::info;
use tracing::trace;
use tracing::warn;

const AUDIO_MODEL: &str = "gpt-4o-mini-transcribe";
const MODEL_AUDIO_SAMPLE_RATE: u32 = 24_000;
const MODEL_AUDIO_CHANNELS: u16 = 1;
const FIRST_TRANSCRIPTION_ATTEMPT_MIN_TIMEOUT: Duration = Duration::from_secs(2);
const FIRST_TRANSCRIPTION_ATTEMPT_MAX_TIMEOUT: Duration = Duration::from_secs(15);
const SECOND_TRANSCRIPTION_ATTEMPT_MIN_TIMEOUT: Duration = Duration::from_secs(4);
const SECOND_TRANSCRIPTION_ATTEMPT_MAX_TIMEOUT: Duration = Duration::from_secs(30);
const FINAL_TRANSCRIPTION_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(60);
const FIRST_TRANSCRIPTION_ATTEMPT_TIMEOUT_PER_AUDIO_SECOND: f32 = 2.0;
const SECOND_TRANSCRIPTION_ATTEMPT_TIMEOUT_PER_AUDIO_SECOND: f32 = 3.0;
const TRANSCRIPTION_ATTEMPT_COUNT: usize = 3;

struct TranscriptionAuthContext {
    mode: AuthMode,
    bearer_token: String,
    chatgpt_account_id: Option<String>,
    chatgpt_base_url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TranscriptionAttempt {
    number: usize,
    timeout: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TranscriptionRetryNotice {
    next_attempt: usize,
    max_attempts: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TranscriptionRetryDecision {
    Retry { delay: Option<Duration> },
    Stop,
}

#[derive(Debug)]
enum TranscriptionRequestError {
    Build(String),
    Timeout(Duration),
    Send(String),
    Status {
        status: reqwest::StatusCode,
        body: String,
        retry_after: Option<Duration>,
    },
    Json(String),
}

impl fmt::Display for TranscriptionRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Build(error) => write!(f, "{error}"),
            Self::Timeout(timeout) => write!(
                f,
                "transcription request timed out after {:.2}s",
                timeout.as_secs_f32()
            ),
            Self::Send(error) => write!(f, "transcription request failed: {error}"),
            Self::Status { status, body, .. } => {
                write!(f, "transcription failed: {status} {body}")
            }
            Self::Json(error) => write!(f, "failed to parse json: {error}"),
        }
    }
}

pub struct RecordedAudio {
    pub data: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
}

pub struct VoiceCapture {
    stream: Option<cpal::Stream>,
    sample_rate: u32,
    channels: u16,
    data: Arc<Mutex<Vec<i16>>>,
    stopped: Arc<AtomicBool>,
    last_peak: Arc<AtomicU16>,
}

impl VoiceCapture {
    pub fn start() -> Result<Self, String> {
        let (device, config) = select_default_input_device_and_config()?;

        let sample_rate = config.sample_rate().0;
        let channels = config.channels();
        let data: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let last_peak = Arc::new(AtomicU16::new(0));

        let stream = build_input_stream(&device, &config, data.clone(), last_peak.clone())?;
        stream
            .play()
            .map_err(|e| format!("failed to start input stream: {e}"))?;

        Ok(Self {
            stream: Some(stream),
            sample_rate,
            channels,
            data,
            stopped,
            last_peak,
        })
    }

    pub fn start_realtime(config: &Config, tx: AppEventSender) -> Result<Self, String> {
        let (device, config) = select_realtime_input_device_and_config(config)?;

        let sample_rate = config.sample_rate().0;
        let channels = config.channels();
        let data: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let last_peak = Arc::new(AtomicU16::new(0));

        let stream = build_realtime_input_stream(
            &device,
            &config,
            sample_rate,
            channels,
            tx,
            last_peak.clone(),
        )?;
        stream
            .play()
            .map_err(|e| format!("failed to start input stream: {e}"))?;

        Ok(Self {
            stream: Some(stream),
            sample_rate,
            channels,
            data,
            stopped,
            last_peak,
        })
    }

    pub fn stop(mut self) -> Result<RecordedAudio, String> {
        // Mark stopped so any metering task can exit cleanly.
        self.stopped.store(true, Ordering::SeqCst);
        // Dropping the stream stops capture.
        self.stream.take();
        let data = self
            .data
            .lock()
            .map_err(|_| "failed to lock audio buffer".to_string())?
            .clone();
        Ok(RecordedAudio {
            data,
            sample_rate: self.sample_rate,
            channels: self.channels,
        })
    }

    pub fn data_arc(&self) -> Arc<Mutex<Vec<i16>>> {
        self.data.clone()
    }

    pub fn stopped_flag(&self) -> Arc<AtomicBool> {
        self.stopped.clone()
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn last_peak_arc(&self) -> Arc<AtomicU16> {
        self.last_peak.clone()
    }
}

pub(crate) struct RecordingMeterState {
    history: VecDeque<char>,
    noise_ema: f64,
    env: f64,
}

impl RecordingMeterState {
    pub(crate) fn new() -> Self {
        let mut history = VecDeque::with_capacity(4);
        while history.len() < 4 {
            history.push_back('⠤');
        }
        Self {
            history,
            noise_ema: 0.02,
            env: 0.0,
        }
    }

    pub(crate) fn next_text(&mut self, peak: u16) -> String {
        const SYMBOLS: [char; 7] = ['⠤', '⠴', '⠶', '⠷', '⡷', '⡿', '⣿'];
        const ALPHA_NOISE: f64 = 0.05;
        const ATTACK: f64 = 0.80;
        const RELEASE: f64 = 0.25;

        let latest_peak = peak as f64 / (i16::MAX as f64);

        if latest_peak > self.env {
            self.env = ATTACK * latest_peak + (1.0 - ATTACK) * self.env;
        } else {
            self.env = RELEASE * latest_peak + (1.0 - RELEASE) * self.env;
        }

        let rms_approx = self.env * 0.7;
        self.noise_ema = (1.0 - ALPHA_NOISE) * self.noise_ema + ALPHA_NOISE * rms_approx;
        let ref_level = self.noise_ema.max(0.01);
        let fast_signal = 0.8 * latest_peak + 0.2 * self.env;
        let target = 2.0f64;
        let raw = (fast_signal / (ref_level * target)).max(0.0);
        let k = 1.6f64;
        let compressed = (raw.ln_1p() / k.ln_1p()).min(1.0);
        let idx = (compressed * (SYMBOLS.len() as f64 - 1.0))
            .round()
            .clamp(0.0, SYMBOLS.len() as f64 - 1.0) as usize;
        let level_char = SYMBOLS[idx];

        if self.history.len() >= 4 {
            self.history.pop_front();
        }
        self.history.push_back(level_char);

        let mut text = String::with_capacity(4);
        for ch in &self.history {
            text.push(*ch);
        }
        text
    }
}

pub fn transcribe_async(
    id: String,
    audio: RecordedAudio,
    context: Option<String>,
    tx: AppEventSender,
) {
    std::thread::spawn(move || {
        const MIN_DURATION_SECONDS: f32 = 1.0;
        let duration_seconds = clip_duration_seconds(&audio);
        if duration_seconds < MIN_DURATION_SECONDS {
            let msg = format!(
                "recording too short ({duration_seconds:.2}s); minimum is {MIN_DURATION_SECONDS:.2}s"
            );
            info!("{msg}");
            tx.send(AppEvent::TranscriptionFailed { id, error: msg });
            return;
        }

        let wav_bytes = match encode_wav_normalized(&audio) {
            Ok(wav_bytes) => wav_bytes,
            Err(err) => {
                error!("failed to encode wav: {err}");
                tx.send(AppEvent::TranscriptionFailed { id, error: err });
                return;
            }
        };

        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(err) => {
                error!("failed to create tokio runtime: {err}");
                tx.send(AppEvent::TranscriptionFailed {
                    id,
                    error: err.to_string(),
                });
                return;
            }
        };

        let retry_tx = tx.clone();
        let retry_id = id.clone();
        let on_retry = move |notice: TranscriptionRetryNotice| {
            retry_tx.send(AppEvent::TranscriptionRetrying {
                id: retry_id.clone(),
                attempt: notice.next_attempt,
                max_attempts: notice.max_attempts,
            });
        };

        match runtime.block_on(transcribe_bytes(
            wav_bytes,
            context,
            duration_seconds,
            on_retry,
        )) {
            Ok(text) => {
                tx.send(AppEvent::TranscriptionComplete { id, text });
                info!("voice transcription succeeded");
            }
            Err(err) => {
                error!("voice transcription error: {err}");
                tx.send(AppEvent::TranscriptionFailed { id, error: err });
            }
        }
    });
}

// -------------------------
// Voice input helpers
// -------------------------

fn select_default_input_device_and_config()
-> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "no input audio device available".to_string())?;
    let config = preferred_input_config(&device)?;
    Ok((device, config))
}

fn select_realtime_input_device_and_config(
    config: &Config,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
    crate::audio_device::select_configured_input_device_and_config(config)
}

fn build_input_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    data: Arc<Mutex<Vec<i16>>>,
    last_peak: Arc<AtomicU16>,
) -> Result<cpal::Stream, String> {
    match config.sample_format() {
        cpal::SampleFormat::F32 => device
            .build_input_stream(
                &config.clone().into(),
                move |input: &[f32], _| {
                    let peak = peak_f32(input);
                    last_peak.store(peak, Ordering::Relaxed);
                    if let Ok(mut buffer) = data.lock() {
                        for &sample in input {
                            buffer.push(f32_to_i16(sample));
                        }
                    }
                },
                move |err| error!("audio input error: {err}"),
                None,
            )
            .map_err(|e| format!("failed to build input stream: {e}")),
        cpal::SampleFormat::I16 => device
            .build_input_stream(
                &config.clone().into(),
                move |input: &[i16], _| {
                    let peak = peak_i16(input);
                    last_peak.store(peak, Ordering::Relaxed);
                    if let Ok(mut buffer) = data.lock() {
                        buffer.extend_from_slice(input);
                    }
                },
                move |err| error!("audio input error: {err}"),
                None,
            )
            .map_err(|e| format!("failed to build input stream: {e}")),
        cpal::SampleFormat::U16 => device
            .build_input_stream(
                &config.clone().into(),
                move |input: &[u16], _| {
                    if let Ok(mut buffer) = data.lock() {
                        let peak = convert_u16_to_i16_and_peak(input, &mut buffer);
                        last_peak.store(peak, Ordering::Relaxed);
                    }
                },
                move |err| error!("audio input error: {err}"),
                None,
            )
            .map_err(|e| format!("failed to build input stream: {e}")),
        _ => Err("unsupported input sample format".to_string()),
    }
}

fn build_realtime_input_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    sample_rate: u32,
    channels: u16,
    tx: AppEventSender,
    last_peak: Arc<AtomicU16>,
) -> Result<cpal::Stream, String> {
    match config.sample_format() {
        cpal::SampleFormat::F32 => device
            .build_input_stream(
                &config.clone().into(),
                move |input: &[f32], _| {
                    let peak = peak_f32(input);
                    last_peak.store(peak, Ordering::Relaxed);
                    let samples = input.iter().copied().map(f32_to_i16).collect::<Vec<_>>();
                    send_realtime_audio_chunk(&tx, samples, sample_rate, channels);
                },
                move |err| error!("audio input error: {err}"),
                None,
            )
            .map_err(|e| format!("failed to build input stream: {e}")),
        cpal::SampleFormat::I16 => device
            .build_input_stream(
                &config.clone().into(),
                move |input: &[i16], _| {
                    let peak = peak_i16(input);
                    last_peak.store(peak, Ordering::Relaxed);
                    send_realtime_audio_chunk(&tx, input.to_vec(), sample_rate, channels);
                },
                move |err| error!("audio input error: {err}"),
                None,
            )
            .map_err(|e| format!("failed to build input stream: {e}")),
        cpal::SampleFormat::U16 => device
            .build_input_stream(
                &config.clone().into(),
                move |input: &[u16], _| {
                    let mut samples = Vec::with_capacity(input.len());
                    let peak = convert_u16_to_i16_and_peak(input, &mut samples);
                    last_peak.store(peak, Ordering::Relaxed);
                    send_realtime_audio_chunk(&tx, samples, sample_rate, channels);
                },
                move |err| error!("audio input error: {err}"),
                None,
            )
            .map_err(|e| format!("failed to build input stream: {e}")),
        _ => Err("unsupported input sample format".to_string()),
    }
}

fn send_realtime_audio_chunk(
    tx: &AppEventSender,
    samples: Vec<i16>,
    sample_rate: u32,
    channels: u16,
) {
    if samples.is_empty() || sample_rate == 0 || channels == 0 {
        return;
    }

    let samples = if sample_rate == MODEL_AUDIO_SAMPLE_RATE && channels == MODEL_AUDIO_CHANNELS {
        samples
    } else {
        convert_pcm16(
            &samples,
            sample_rate,
            channels,
            MODEL_AUDIO_SAMPLE_RATE,
            MODEL_AUDIO_CHANNELS,
        )
    };
    if samples.is_empty() {
        return;
    }

    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in &samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let samples_per_channel = (samples.len() / usize::from(MODEL_AUDIO_CHANNELS)) as u32;

    tx.realtime_conversation_audio(ConversationAudioParams {
        frame: RealtimeAudioFrame {
            data: encoded,
            sample_rate: MODEL_AUDIO_SAMPLE_RATE,
            num_channels: MODEL_AUDIO_CHANNELS,
            samples_per_channel: Some(samples_per_channel),
            item_id: None,
        },
    });
}

#[inline]
fn f32_abs_to_u16(x: f32) -> u16 {
    let peak_u = (x.abs().min(1.0) * i16::MAX as f32) as i32;
    peak_u.max(0) as u16
}

#[inline]
fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

fn peak_f32(input: &[f32]) -> u16 {
    let mut peak: f32 = 0.0;
    for &s in input {
        let a = s.abs();
        if a > peak {
            peak = a;
        }
    }
    f32_abs_to_u16(peak)
}

fn peak_i16(input: &[i16]) -> u16 {
    let mut peak: i32 = 0;
    for &s in input {
        let a = (s as i32).unsigned_abs() as i32;
        if a > peak {
            peak = a;
        }
    }
    peak as u16
}

fn convert_u16_to_i16_and_peak(input: &[u16], out: &mut Vec<i16>) -> u16 {
    let mut peak: i32 = 0;
    for &s in input {
        let v_i16 = (s as i32 - 32768) as i16;
        let a = (v_i16 as i32).unsigned_abs() as i32;
        if a > peak {
            peak = a;
        }
        out.push(v_i16);
    }
    peak as u16
}

// -------------------------
// Realtime audio playback helpers
// -------------------------

pub(crate) struct RealtimeAudioPlayer {
    _stream: cpal::Stream,
    queue: Arc<Mutex<VecDeque<i16>>>,
    output_sample_rate: u32,
    output_channels: u16,
}

impl RealtimeAudioPlayer {
    pub(crate) fn start(config: &Config) -> Result<Self, String> {
        let (device, config) =
            crate::audio_device::select_configured_output_device_and_config(config)?;
        let output_sample_rate = config.sample_rate().0;
        let output_channels = config.channels();
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let stream = build_output_stream(&device, &config, Arc::clone(&queue))?;
        stream
            .play()
            .map_err(|e| format!("failed to start output stream: {e}"))?;
        Ok(Self {
            _stream: stream,
            queue,
            output_sample_rate,
            output_channels,
        })
    }

    pub(crate) fn enqueue_frame(&self, frame: &RealtimeAudioFrame) -> Result<(), String> {
        if frame.num_channels == 0 || frame.sample_rate == 0 {
            return Err("invalid realtime audio frame format".to_string());
        }
        let raw_bytes = base64::engine::general_purpose::STANDARD
            .decode(&frame.data)
            .map_err(|e| format!("failed to decode realtime audio: {e}"))?;
        if raw_bytes.len() % 2 != 0 {
            return Err("realtime audio frame had odd byte length".to_string());
        }
        let mut pcm = Vec::with_capacity(raw_bytes.len() / 2);
        for pair in raw_bytes.chunks_exact(2) {
            pcm.push(i16::from_le_bytes([pair[0], pair[1]]));
        }
        let converted = convert_pcm16(
            &pcm,
            frame.sample_rate,
            frame.num_channels,
            self.output_sample_rate,
            self.output_channels,
        );
        if converted.is_empty() {
            return Ok(());
        }
        let mut guard = self
            .queue
            .lock()
            .map_err(|_| "failed to lock output audio queue".to_string())?;
        // TODO(aibrahim): Cap or trim this queue if we observe producer bursts outrunning playback.
        guard.extend(converted);
        Ok(())
    }

    pub(crate) fn clear(&self) {
        if let Ok(mut guard) = self.queue.lock() {
            guard.clear();
        }
    }
}

fn build_output_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    queue: Arc<Mutex<VecDeque<i16>>>,
) -> Result<cpal::Stream, String> {
    let config_any: cpal::StreamConfig = config.clone().into();
    match config.sample_format() {
        cpal::SampleFormat::F32 => device
            .build_output_stream(
                &config_any,
                move |output: &mut [f32], _| fill_output_f32(output, &queue),
                move |err| error!("audio output error: {err}"),
                None,
            )
            .map_err(|e| format!("failed to build f32 output stream: {e}")),
        cpal::SampleFormat::I16 => device
            .build_output_stream(
                &config_any,
                move |output: &mut [i16], _| fill_output_i16(output, &queue),
                move |err| error!("audio output error: {err}"),
                None,
            )
            .map_err(|e| format!("failed to build i16 output stream: {e}")),
        cpal::SampleFormat::U16 => device
            .build_output_stream(
                &config_any,
                move |output: &mut [u16], _| fill_output_u16(output, &queue),
                move |err| error!("audio output error: {err}"),
                None,
            )
            .map_err(|e| format!("failed to build u16 output stream: {e}")),
        other => Err(format!("unsupported output sample format: {other:?}")),
    }
}

fn fill_output_i16(output: &mut [i16], queue: &Arc<Mutex<VecDeque<i16>>>) {
    if let Ok(mut guard) = queue.lock() {
        for sample in output {
            *sample = guard.pop_front().unwrap_or(0);
        }
        return;
    }
    output.fill(0);
}

fn fill_output_f32(output: &mut [f32], queue: &Arc<Mutex<VecDeque<i16>>>) {
    if let Ok(mut guard) = queue.lock() {
        for sample in output {
            let v = guard.pop_front().unwrap_or(0);
            *sample = (v as f32) / (i16::MAX as f32);
        }
        return;
    }
    output.fill(0.0);
}

fn fill_output_u16(output: &mut [u16], queue: &Arc<Mutex<VecDeque<i16>>>) {
    if let Ok(mut guard) = queue.lock() {
        for sample in output {
            let v = guard.pop_front().unwrap_or(0);
            *sample = (v as i32 + 32768).clamp(0, u16::MAX as i32) as u16;
        }
        return;
    }
    output.fill(32768);
}

fn convert_pcm16(
    input: &[i16],
    input_sample_rate: u32,
    input_channels: u16,
    output_sample_rate: u32,
    output_channels: u16,
) -> Vec<i16> {
    if input.is_empty() || input_channels == 0 || output_channels == 0 {
        return Vec::new();
    }

    let in_channels = input_channels as usize;
    let out_channels = output_channels as usize;
    let in_frames = input.len() / in_channels;
    if in_frames == 0 {
        return Vec::new();
    }

    let out_frames = if input_sample_rate == output_sample_rate {
        in_frames
    } else {
        (((in_frames as u64) * (output_sample_rate as u64)) / (input_sample_rate as u64)).max(1)
            as usize
    };

    let mut out = Vec::with_capacity(out_frames.saturating_mul(out_channels));
    for out_frame_idx in 0..out_frames {
        let src_frame_idx = if out_frames <= 1 || in_frames <= 1 {
            0
        } else {
            ((out_frame_idx as u64) * ((in_frames - 1) as u64) / ((out_frames - 1) as u64)) as usize
        };
        let src_start = src_frame_idx.saturating_mul(in_channels);
        let src = &input[src_start..src_start + in_channels];
        match (in_channels, out_channels) {
            (1, 1) => out.push(src[0]),
            (1, n) => {
                for _ in 0..n {
                    out.push(src[0]);
                }
            }
            (n, 1) if n >= 2 => {
                let sum: i32 = src.iter().map(|s| *s as i32).sum();
                out.push((sum / (n as i32)) as i16);
            }
            (n, m) if n == m => out.extend_from_slice(src),
            (n, m) if n > m => out.extend_from_slice(&src[..m]),
            (n, m) => {
                out.extend_from_slice(src);
                let last = *src.last().unwrap_or(&0);
                for _ in n..m {
                    out.push(last);
                }
            }
        }
    }
    out
}

// -------------------------
// Transcription helpers
// -------------------------

fn clip_duration_seconds(audio: &RecordedAudio) -> f32 {
    let total_samples = audio.data.len() as f32;
    let samples_per_second = (audio.sample_rate as f32) * (audio.channels as f32);
    if samples_per_second > 0.0 {
        total_samples / samples_per_second
    } else {
        0.0
    }
}

fn encode_wav_normalized(audio: &RecordedAudio) -> Result<Vec<u8>, String> {
    let converted;
    let (channels, sample_rate, segment) =
        if audio.channels == MODEL_AUDIO_CHANNELS && audio.sample_rate == MODEL_AUDIO_SAMPLE_RATE {
            (audio.channels, audio.sample_rate, audio.data.as_slice())
        } else {
            converted = convert_pcm16(
                &audio.data,
                audio.sample_rate,
                audio.channels,
                MODEL_AUDIO_SAMPLE_RATE,
                MODEL_AUDIO_CHANNELS,
            );
            (
                MODEL_AUDIO_CHANNELS,
                MODEL_AUDIO_SAMPLE_RATE,
                converted.as_slice(),
            )
        };

    let spec = WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut wav_bytes = Vec::new();
    let mut cursor = Cursor::new(&mut wav_bytes);
    let mut writer =
        WavWriter::new(&mut cursor, spec).map_err(|_| "failed to create wav writer".to_string())?;

    let peak_abs = segment
        .iter()
        .map(|sample| (i32::from(*sample)).unsigned_abs() as i32)
        .max()
        .unwrap_or(0);
    let target = (i16::MAX as f32) * 0.9;
    let gain = if peak_abs > 0 {
        target / (peak_abs as f32)
    } else {
        1.0
    };

    for &sample in segment {
        let normalized = ((sample as f32) * gain)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        writer
            .write_sample(normalized)
            .map_err(|_| "failed writing wav sample".to_string())?;
    }
    writer
        .finalize()
        .map_err(|_| "failed to finalize wav".to_string())?;
    Ok(wav_bytes)
}

fn normalize_chatgpt_base_url(input: &str) -> String {
    let mut base_url = input.to_string();
    while base_url.ends_with('/') {
        base_url.pop();
    }
    if (base_url.starts_with("https://chatgpt.com")
        || base_url.starts_with("https://chat.openai.com"))
        && !base_url.contains("/backend-api")
    {
        base_url = format!("{base_url}/backend-api");
    }
    base_url
}

async fn resolve_auth() -> Result<TranscriptionAuthContext, String> {
    let codex_home = find_codex_home().map_err(|e| format!("failed to find codex home: {e}"))?;
    let auth = CodexAuth::from_auth_storage(&codex_home, AuthCredentialsStoreMode::Auto)
        .await
        .map_err(|e| format!("failed to read auth.json: {e}"))?
        .ok_or_else(|| "No Codex auth is configured; please run `codex login`".to_string())?;

    let chatgpt_account_id = auth.get_account_id();
    let bearer_token = auth
        .get_token()
        .map_err(|e| format!("failed to get auth token: {e}"))?;
    let config = Config::load_with_cli_overrides(Vec::new())
        .await
        .map_err(|e| format!("failed to load config: {e}"))?;
    Ok(TranscriptionAuthContext {
        mode: auth.api_auth_mode(),
        bearer_token,
        chatgpt_account_id,
        chatgpt_base_url: normalize_chatgpt_base_url(&config.chatgpt_base_url),
    })
}

async fn transcribe_bytes(
    wav_bytes: Vec<u8>,
    context: Option<String>,
    duration_seconds: f32,
    on_retry: impl Fn(TranscriptionRetryNotice),
) -> Result<String, String> {
    let started_at = Instant::now();
    let auth = resolve_auth().await?;
    let auth_elapsed = started_at.elapsed();
    let client = build_reqwest_client_with_custom_ca(reqwest::Client::builder())
        .map_err(|error| format!("failed to build transcription HTTP client: {error}"))?;
    let audio_bytes = wav_bytes.len();
    let prompt_for_log = context.as_deref().unwrap_or("").to_string();
    let audio_kib = audio_bytes as f32 / 1024.0;
    let mode = auth.mode;
    trace!(
        "preparing transcription request: mode={mode:?} duration={duration_seconds:.2}s audio={audio_kib:.1}KiB prompt={prompt_for_log}"
    );
    let value = send_transcription_request_with_retries(
        &client,
        &auth,
        &wav_bytes,
        context.as_deref(),
        TranscriptionRequestMetrics {
            mode,
            duration_seconds,
            audio_kib,
            auth_elapsed,
            started_at,
        },
        on_retry,
    )
    .await
    .map_err(|error| error.to_string())?;

    let text = value
        .get("text")
        .and_then(|text| text.as_str())
        .unwrap_or("")
        .to_string();

    if text.is_empty() {
        Err("empty transcription result".to_string())
    } else {
        Ok(text)
    }
}

#[derive(Clone, Copy)]
struct TranscriptionRequestMetrics {
    mode: AuthMode,
    duration_seconds: f32,
    audio_kib: f32,
    auth_elapsed: Duration,
    started_at: Instant,
}

fn transcription_request_attempts(duration_seconds: f32) -> [TranscriptionAttempt; 3] {
    [
        TranscriptionAttempt {
            number: 1,
            timeout: scaled_transcription_request_timeout(
                duration_seconds,
                FIRST_TRANSCRIPTION_ATTEMPT_MIN_TIMEOUT,
                FIRST_TRANSCRIPTION_ATTEMPT_MAX_TIMEOUT,
                FIRST_TRANSCRIPTION_ATTEMPT_TIMEOUT_PER_AUDIO_SECOND,
            ),
        },
        TranscriptionAttempt {
            number: 2,
            timeout: scaled_transcription_request_timeout(
                duration_seconds,
                SECOND_TRANSCRIPTION_ATTEMPT_MIN_TIMEOUT,
                SECOND_TRANSCRIPTION_ATTEMPT_MAX_TIMEOUT,
                SECOND_TRANSCRIPTION_ATTEMPT_TIMEOUT_PER_AUDIO_SECOND,
            ),
        },
        TranscriptionAttempt {
            number: 3,
            timeout: FINAL_TRANSCRIPTION_ATTEMPT_TIMEOUT,
        },
    ]
}

fn scaled_transcription_request_timeout(
    duration_seconds: f32,
    min_timeout: Duration,
    max_timeout: Duration,
    timeout_per_audio_second: f32,
) -> Duration {
    let scaled_timeout = if duration_seconds.is_finite() && duration_seconds > 0.0 {
        Duration::from_secs_f32(duration_seconds * timeout_per_audio_second)
    } else {
        min_timeout
    };

    scaled_timeout.clamp(min_timeout, max_timeout)
}

fn build_transcription_request(
    client: &reqwest::Client,
    auth: &TranscriptionAuthContext,
    wav_bytes: &[u8],
    context: Option<&str>,
) -> Result<(String, reqwest::RequestBuilder), TranscriptionRequestError> {
    if matches!(auth.mode, AuthMode::Chatgpt | AuthMode::ChatgptAuthTokens) {
        let part = reqwest::multipart::Part::bytes(wav_bytes.to_vec())
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|error| {
                TranscriptionRequestError::Build(format!("failed to set mime: {error}"))
            })?;
        let form = reqwest::multipart::Form::new().part("file", part);
        let endpoint = format!("{}/transcribe", auth.chatgpt_base_url);
        let request = if let Some(account_id) = &auth.chatgpt_account_id {
            client
                .post(&endpoint)
                .bearer_auth(&auth.bearer_token)
                .multipart(form)
                .header("User-Agent", get_codex_user_agent())
                .header("ChatGPT-Account-Id", account_id.as_str())
        } else {
            client
                .post(&endpoint)
                .bearer_auth(&auth.bearer_token)
                .multipart(form)
                .header("User-Agent", get_codex_user_agent())
        };
        Ok((endpoint, request))
    } else {
        let part = reqwest::multipart::Part::bytes(wav_bytes.to_vec())
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|error| {
                TranscriptionRequestError::Build(format!("failed to set mime: {error}"))
            })?;
        let form = if let Some(context) = context {
            reqwest::multipart::Form::new()
                .text("model", AUDIO_MODEL)
                .part("file", part)
                .text("prompt", context.to_string())
        } else {
            reqwest::multipart::Form::new()
                .text("model", AUDIO_MODEL)
                .part("file", part)
        };
        let endpoint = "https://api.openai.com/v1/audio/transcriptions".to_string();
        Ok((
            endpoint.clone(),
            client
                .post(&endpoint)
                .bearer_auth(&auth.bearer_token)
                .multipart(form)
                .header("User-Agent", get_codex_user_agent()),
        ))
    }
}

async fn send_transcription_request_with_retries(
    client: &reqwest::Client,
    auth: &TranscriptionAuthContext,
    wav_bytes: &[u8],
    context: Option<&str>,
    metrics: TranscriptionRequestMetrics,
    on_retry: impl Fn(TranscriptionRetryNotice),
) -> Result<serde_json::Value, TranscriptionRequestError> {
    let attempts = transcription_request_attempts(metrics.duration_seconds);
    let mut last_error = None;

    for attempt_index in 0..attempts.len() {
        let attempt = attempts[attempt_index];
        let next_attempt = attempts.get(attempt_index + 1).copied();
        let (endpoint, request) = build_transcription_request(client, auth, wav_bytes, context)?;
        info!(
            "sending voice transcription request: mode={:?} endpoint={endpoint} attempt={}/{} duration={:.2}s audio={:.1}KiB timeout={:.2}s auth_config_elapsed_ms={}",
            metrics.mode,
            attempt.number,
            TRANSCRIPTION_ATTEMPT_COUNT,
            metrics.duration_seconds,
            metrics.audio_kib,
            attempt.timeout.as_secs_f32(),
            metrics.auth_elapsed.as_millis()
        );

        let request_started_at = Instant::now();
        match send_transcription_request_with_timeout(request, attempt.timeout).await {
            Ok(value) => {
                let request_elapsed = request_started_at.elapsed();
                info!(
                    "voice transcription response parsed: attempt={}/{} request_elapsed_ms={} total_elapsed_ms={}",
                    attempt.number,
                    TRANSCRIPTION_ATTEMPT_COUNT,
                    request_elapsed.as_millis(),
                    metrics.started_at.elapsed().as_millis()
                );
                return Ok(value);
            }
            Err(error) => {
                let request_elapsed = request_started_at.elapsed();
                match transcription_retry_decision(&error, next_attempt) {
                    TranscriptionRetryDecision::Retry { delay } => {
                        warn!(
                            "voice transcription attempt failed; retrying: attempt={}/{} request_elapsed_ms={} total_elapsed_ms={} error={error}",
                            attempt.number,
                            TRANSCRIPTION_ATTEMPT_COUNT,
                            request_elapsed.as_millis(),
                            metrics.started_at.elapsed().as_millis()
                        );
                        on_retry(TranscriptionRetryNotice {
                            next_attempt: attempt.number + 1,
                            max_attempts: TRANSCRIPTION_ATTEMPT_COUNT,
                        });
                        if let Some(delay) = delay {
                            info!(
                                "waiting before voice transcription retry: retry_after_ms={}",
                                delay.as_millis()
                            );
                            tokio::time::sleep(delay).await;
                        }
                        last_error = Some(error);
                    }
                    TranscriptionRetryDecision::Stop => {
                        warn!(
                            "voice transcription attempt failed; giving up: attempt={}/{} request_elapsed_ms={} total_elapsed_ms={} error={error}",
                            attempt.number,
                            TRANSCRIPTION_ATTEMPT_COUNT,
                            request_elapsed.as_millis(),
                            metrics.started_at.elapsed().as_millis()
                        );
                        return Err(error);
                    }
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        TranscriptionRequestError::Build("no transcription attempts configured".to_string())
    }))
}

fn transcription_retry_decision(
    error: &TranscriptionRequestError,
    next_attempt: Option<TranscriptionAttempt>,
) -> TranscriptionRetryDecision {
    let Some(next_attempt) = next_attempt else {
        return TranscriptionRetryDecision::Stop;
    };

    match error {
        TranscriptionRequestError::Timeout(_) | TranscriptionRequestError::Send(_) => {
            TranscriptionRetryDecision::Retry { delay: None }
        }
        TranscriptionRequestError::Status { status, .. }
            if matches!(
                *status,
                reqwest::StatusCode::BAD_GATEWAY
                    | reqwest::StatusCode::SERVICE_UNAVAILABLE
                    | reqwest::StatusCode::GATEWAY_TIMEOUT
            ) =>
        {
            TranscriptionRetryDecision::Retry { delay: None }
        }
        TranscriptionRequestError::Status {
            status,
            retry_after,
            ..
        } if *status == reqwest::StatusCode::TOO_MANY_REQUESTS => match retry_after {
            Some(delay) if *delay <= next_attempt.timeout => TranscriptionRetryDecision::Retry {
                delay: Some(*delay),
            },
            Some(_) => TranscriptionRetryDecision::Stop,
            None => TranscriptionRetryDecision::Retry { delay: None },
        },
        TranscriptionRequestError::Build(_)
        | TranscriptionRequestError::Status { .. }
        | TranscriptionRequestError::Json(_) => TranscriptionRetryDecision::Stop,
    }
}

fn retry_after_duration(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
}

async fn send_transcription_request_with_timeout(
    request: reqwest::RequestBuilder,
    timeout: Duration,
) -> Result<serde_json::Value, TranscriptionRequestError> {
    // Use an explicit async deadline because reqwest otherwise has no end-to-end request timeout
    // on this client builder.
    with_transcription_timeout(send_transcription_request(request), timeout).await
}

async fn with_transcription_timeout<F, T>(
    future: F,
    timeout: Duration,
) -> Result<T, TranscriptionRequestError>
where
    F: Future<Output = Result<T, TranscriptionRequestError>>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result,
        Err(_) => Err(TranscriptionRequestError::Timeout(timeout)),
    }
}

async fn send_transcription_request(
    request: reqwest::RequestBuilder,
) -> Result<serde_json::Value, TranscriptionRequestError> {
    let response = request
        .send()
        .await
        .map_err(|error| TranscriptionRequestError::Send(error.to_string()))?;
    if !response.status().is_success() {
        let status = response.status();
        let retry_after = retry_after_duration(response.headers());
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<failed to read body>".to_string());
        return Err(TranscriptionRequestError::Status {
            status,
            body,
            retry_after,
        });
    }

    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|error| TranscriptionRequestError::Json(error.to_string()))?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::RecordedAudio;
    use super::TranscriptionAttempt;
    use super::TranscriptionRequestError;
    use super::TranscriptionRetryDecision;
    use super::convert_pcm16;
    use super::encode_wav_normalized;
    use super::send_transcription_request_with_timeout;
    use super::transcription_request_attempts;
    use super::transcription_retry_decision;
    use pretty_assertions::assert_eq;
    use std::io::Cursor;
    use std::net::Ipv4Addr;
    use std::time::Duration;
    use tokio::time;

    #[test]
    fn convert_pcm16_downmixes_and_resamples_for_model_input() {
        let input = vec![100, 300, 200, 400, 500, 700, 600, 800];
        let converted = convert_pcm16(
            &input, /*input_sample_rate*/ 48_000, /*input_channels*/ 2,
            /*output_sample_rate*/ 24_000, /*output_channels*/ 1,
        );
        assert_eq!(converted, vec![200, 700]);
    }

    #[test]
    fn encode_wav_normalized_outputs_24khz_mono_audio() {
        let audio = RecordedAudio {
            data: vec![100, -100, 200, -200],
            sample_rate: 48_000,
            channels: 2,
        };

        let bytes = encode_wav_normalized(&audio).unwrap();
        let reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        let spec = reader.spec();

        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 24_000);
        assert_eq!(spec.bits_per_sample, 16);
    }

    #[test]
    fn transcription_request_attempts_scale_with_audio_duration() {
        let actual = [0.0, f32::NAN, 1.0, 5.0, 10.0, 20.0]
            .into_iter()
            .map(|duration_seconds| {
                transcription_request_attempts(duration_seconds).map(|attempt| attempt.timeout)
            })
            .collect::<Vec<_>>();

        assert_eq!(
            actual,
            vec![
                [
                    Duration::from_secs(2),
                    Duration::from_secs(4),
                    Duration::from_secs(60),
                ],
                [
                    Duration::from_secs(2),
                    Duration::from_secs(4),
                    Duration::from_secs(60),
                ],
                [
                    Duration::from_secs(2),
                    Duration::from_secs(4),
                    Duration::from_secs(60),
                ],
                [
                    Duration::from_secs(10),
                    Duration::from_secs(15),
                    Duration::from_secs(60),
                ],
                [
                    Duration::from_secs(15),
                    Duration::from_secs(30),
                    Duration::from_secs(60),
                ],
                [
                    Duration::from_secs(15),
                    Duration::from_secs(30),
                    Duration::from_secs(60),
                ],
            ]
        );
    }

    #[test]
    fn transcription_retry_decision_retries_only_transient_failures() {
        let next_attempt = Some(TranscriptionAttempt {
            number: 2,
            timeout: Duration::from_secs(4),
        });

        assert_eq!(
            transcription_retry_decision(
                &TranscriptionRequestError::Timeout(Duration::from_secs(2)),
                next_attempt
            ),
            TranscriptionRetryDecision::Retry { delay: None }
        );
        assert_eq!(
            transcription_retry_decision(
                &TranscriptionRequestError::Send("connection reset".to_string()),
                next_attempt
            ),
            TranscriptionRetryDecision::Retry { delay: None }
        );
        assert_eq!(
            transcription_retry_decision(
                &TranscriptionRequestError::Status {
                    status: reqwest::StatusCode::BAD_GATEWAY,
                    body: "bad gateway".to_string(),
                    retry_after: None,
                },
                next_attempt
            ),
            TranscriptionRetryDecision::Retry { delay: None }
        );
        assert_eq!(
            transcription_retry_decision(
                &TranscriptionRequestError::Status {
                    status: reqwest::StatusCode::TOO_MANY_REQUESTS,
                    body: "slow down".to_string(),
                    retry_after: Some(Duration::from_secs(3)),
                },
                next_attempt
            ),
            TranscriptionRetryDecision::Retry {
                delay: Some(Duration::from_secs(3))
            }
        );
        assert_eq!(
            transcription_retry_decision(
                &TranscriptionRequestError::Status {
                    status: reqwest::StatusCode::TOO_MANY_REQUESTS,
                    body: "slow down".to_string(),
                    retry_after: Some(Duration::from_secs(5)),
                },
                next_attempt
            ),
            TranscriptionRetryDecision::Stop
        );
        assert_eq!(
            transcription_retry_decision(
                &TranscriptionRequestError::Status {
                    status: reqwest::StatusCode::UNAUTHORIZED,
                    body: "no".to_string(),
                    retry_after: None,
                },
                next_attempt
            ),
            TranscriptionRetryDecision::Stop
        );
        assert_eq!(
            transcription_retry_decision(
                &TranscriptionRequestError::Json("invalid".to_string()),
                next_attempt
            ),
            TranscriptionRetryDecision::Stop
        );
        assert_eq!(
            transcription_retry_decision(
                &TranscriptionRequestError::Timeout(Duration::from_secs(60)),
                None
            ),
            TranscriptionRetryDecision::Stop
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn transcription_request_times_out_unresponsive_endpoint() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let url = format!("http://{}/transcribe", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });

        let timeout = Duration::from_secs(10);
        let task = tokio::spawn(send_transcription_request_with_timeout(
            reqwest::Client::new().get(url),
            timeout,
        ));
        tokio::task::yield_now().await;
        time::advance(timeout).await;

        let err = time::timeout(Duration::from_millis(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        server.abort();

        assert_eq!(
            err.to_string(),
            "transcription request timed out after 10.00s"
        );
    }
}
