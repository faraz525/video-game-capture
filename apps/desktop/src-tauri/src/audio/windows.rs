use super::{AudioBuffer, AudioCapture, AudioConfig, AudioError};
use crate::sync::clock::SyncClock;
use log::{error, info, warn};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode};

/// How often to check for audio device changes (in poll iterations).
/// At ~10ms poll interval, 200 iterations ≈ 2 seconds.
const DEVICE_CHECK_INTERVAL: u64 = 200;

/// Number of silence buffers to inject on device switch (each ~20ms at 48kHz).
/// 5 buffers = 100ms of silence, bridging the gap during device re-init.
const SILENCE_BUFFERS_ON_SWITCH: usize = 5;

/// Samples per silence buffer (20ms at 48kHz stereo).
const SILENCE_BUFFER_SAMPLES: usize = 48000 / 50 * 2; // 1920 samples

/// Windows audio capture using WASAPI loopback mode.
///
/// Captures system audio output (what the user hears) by opening
/// the default render device in loopback mode. Uses polling (not
/// event-driven) because WASAPI loopback doesn't support event mode.
///
/// Handles audio device switches gracefully: detects when the default
/// render device changes, injects silence buffers to bridge the gap,
/// and re-opens capture on the new device.
pub struct WindowsAudioCapture {
    clock: SyncClock,
    running: Arc<Mutex<bool>>,
    buffers: Arc<Mutex<VecDeque<AudioBuffer>>>,
    config: Option<AudioConfig>,
}

impl WindowsAudioCapture {
    pub fn new(clock: SyncClock) -> Self {
        Self {
            clock,
            running: Arc::new(Mutex::new(false)),
            buffers: Arc::new(Mutex::new(VecDeque::new())),
            config: None,
        }
    }
}

impl AudioCapture for WindowsAudioCapture {
    fn start(&mut self, config: AudioConfig) -> Result<(), AudioError> {
        {
            let running = self.running.lock().map_err(|e| {
                AudioError::Platform(format!("Failed to lock running flag: {e}"))
            })?;
            if *running {
                return Err(AudioError::AlreadyRunning);
            }
        }

        self.config = Some(config.clone());

        let running = Arc::clone(&self.running);
        let buffers = Arc::clone(&self.buffers);
        let clock = self.clock.clone();

        {
            let mut r = running.lock().map_err(|e| {
                AudioError::Platform(format!("Failed to lock running flag: {e}"))
            })?;
            *r = true;
        }

        thread::spawn(move || {
            if let Err(e) = wasapi_capture_loop(running.clone(), buffers, clock, config) {
                error!("WASAPI capture error: {e}");
                if let Ok(mut r) = running.lock() {
                    *r = false;
                }
            }
        });

        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        let mut running = self.running.lock().map_err(|e| {
            AudioError::Platform(format!("Failed to lock running flag: {e}"))
        })?;
        if !*running {
            return Err(AudioError::NotStarted);
        }
        *running = false;
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.lock().map(|r| *r).unwrap_or(false)
    }

    fn poll_buffer(&mut self) -> Result<Option<AudioBuffer>, AudioError> {
        let running = self.running.lock().map_err(|e| {
            AudioError::Platform(format!("Failed to lock running flag: {e}"))
        })?;
        if !*running {
            return Err(AudioError::NotStarted);
        }
        drop(running);

        let mut buffers = self.buffers.lock().map_err(|e| {
            AudioError::Platform(format!("Failed to lock buffers: {e}"))
        })?;

        Ok(buffers.pop_front())
    }
}

/// Get the device ID of the default render device.
fn get_default_device_id(enumerator: &DeviceEnumerator) -> Option<String> {
    enumerator
        .get_default_device(&Direction::Render)
        .ok()
        .and_then(|d| d.get_id().ok())
}

/// Inject silence buffers to bridge audio gaps during device switches.
fn inject_silence(
    buffers: &Arc<Mutex<VecDeque<AudioBuffer>>>,
    clock: &SyncClock,
    channels: u16,
    sample_rate: u32,
) {
    let samples_per_buffer = (sample_rate as usize / 50) * channels as usize;
    if let Ok(mut bufs) = buffers.lock() {
        for _ in 0..SILENCE_BUFFERS_ON_SWITCH {
            bufs.push_back(AudioBuffer {
                timestamp_us: clock.now_us(),
                channels,
                sample_rate,
                samples: vec![0.0; samples_per_buffer],
            });
        }
    }
}

/// Start capture on a WASAPI device. Returns the audio client, capture client,
/// sample type, sample rate, channels, and bits per sample.
fn start_device_capture(
    enumerator: &DeviceEnumerator,
) -> Result<
    (
        wasapi::AudioClient,
        wasapi::AudioCaptureClient,
        SampleType,
        u32,
        u16,
        u16,
    ),
    AudioError,
> {
    let device = enumerator
        .get_default_device(&Direction::Render)
        .map_err(|e| AudioError::Platform(format!("Failed to get render device: {e}")))?;

    let mut audio_client = device
        .get_iaudioclient()
        .map_err(|e| AudioError::Platform(format!("Failed to get audio client: {e}")))?;

    let mix_format = audio_client
        .get_mixformat()
        .map_err(|e| AudioError::Platform(format!("Failed to get mix format: {e}")))?;

    let sample_rate = mix_format.get_samplespersec();
    let channels = mix_format.get_nchannels() as u16;
    let bits_per_sample = mix_format.get_bitspersample();
    let sample_type = mix_format
        .get_subformat()
        .map_err(|e| AudioError::Platform(format!("Failed to get sample type: {e}")))?;

    let mode = StreamMode::PollingShared {
        autoconvert: true,
        buffer_duration_hns: 0,
    };
    audio_client
        .initialize_client(&mix_format, &Direction::Render, &mode)
        .map_err(|e| AudioError::Platform(format!("Failed to init audio client: {e}")))?;

    let capture_client = audio_client
        .get_audiocaptureclient()
        .map_err(|e| AudioError::Platform(format!("Failed to get capture client: {e}")))?;

    audio_client
        .start_stream()
        .map_err(|e| AudioError::Platform(format!("Failed to start stream: {e}")))?;

    Ok((
        audio_client,
        capture_client,
        sample_type,
        sample_rate,
        channels,
        bits_per_sample,
    ))
}

/// The WASAPI capture loop running on a dedicated thread.
fn wasapi_capture_loop(
    running: Arc<Mutex<bool>>,
    buffers: Arc<Mutex<VecDeque<AudioBuffer>>>,
    clock: SyncClock,
    _config: AudioConfig,
) -> Result<(), AudioError> {
    wasapi::initialize_mta()
        .ok()
        .map_err(|e| AudioError::Platform(format!("COM init failed: {e}")))?;

    let enumerator = DeviceEnumerator::new()
        .map_err(|e| AudioError::Platform(format!("Failed to create enumerator: {e}")))?;

    let (mut audio_client, mut capture_client, mut sample_type, mut sample_rate, mut channels, mut bits_per_sample) =
        start_device_capture(&enumerator)?;

    let mut current_device_id = get_default_device_id(&enumerator);

    let poll_interval = Duration::from_millis(10);
    let mut sample_queue: VecDeque<u8> = VecDeque::new();
    let mut poll_count: u64 = 0;

    loop {
        {
            let r = running.lock().unwrap_or_else(|e| e.into_inner());
            if !*r {
                break;
            }
        }

        // Periodic device change detection
        poll_count += 1;
        if poll_count % DEVICE_CHECK_INTERVAL == 0 {
            let new_device_id = get_default_device_id(&enumerator);
            if new_device_id != current_device_id {
                info!(
                    "Audio device changed: {:?} → {:?}",
                    current_device_id, new_device_id,
                );

                // Stop old stream
                if let Err(e) = audio_client.stop_stream() {
                    warn!("Error stopping old audio stream: {e}");
                }

                // Inject silence to bridge the gap
                inject_silence(&buffers, &clock, channels, sample_rate);

                // Start capture on new device
                match start_device_capture(&enumerator) {
                    Ok((new_client, new_capture, new_type, new_rate, new_ch, new_bps)) => {
                        audio_client = new_client;
                        capture_client = new_capture;
                        sample_type = new_type;
                        sample_rate = new_rate;
                        channels = new_ch;
                        bits_per_sample = new_bps;
                        current_device_id = new_device_id;
                        sample_queue.clear();
                        info!("Audio capture switched to new device ({}Hz, {}ch, {}bit)",
                            sample_rate, channels, bits_per_sample);
                    }
                    Err(e) => {
                        error!("Failed to start capture on new audio device: {e}");
                        // Keep running — device might come back
                    }
                }
            }
        }

        // Read available data into the queue
        match capture_client.read_from_device_to_deque(&mut sample_queue) {
            Ok(_) => {
                if !sample_queue.is_empty() {
                    let raw_bytes: Vec<u8> = sample_queue.drain(..).collect();
                    let samples = convert_to_f32(&raw_bytes, &sample_type, bits_per_sample);
                    if !samples.is_empty() {
                        let buffer = AudioBuffer {
                            timestamp_us: clock.now_us(),
                            channels,
                            sample_rate,
                            samples,
                        };

                        if let Ok(mut bufs) = buffers.lock() {
                            bufs.push_back(buffer);
                        }
                    }
                }
            }
            Err(e) => {
                error!("WASAPI read error: {e}");
            }
        }

        thread::sleep(poll_interval);
    }

    audio_client
        .stop_stream()
        .map_err(|e| AudioError::Platform(format!("Failed to stop stream: {e}")))?;

    Ok(())
}

/// Convert raw WASAPI bytes to f32 samples based on the sample type.
///
/// Uses `bits_per_sample` to correctly handle Int format (16-bit vs 32-bit)
/// instead of assuming 16-bit.
fn convert_to_f32(raw: &[u8], sample_type: &SampleType, bits_per_sample: u16) -> Vec<f32> {
    match sample_type {
        SampleType::Float => {
            raw.chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect()
        }
        SampleType::Int => {
            match bits_per_sample {
                16 => {
                    raw.chunks_exact(2)
                        .map(|chunk| {
                            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                            sample as f32 / i16::MAX as f32
                        })
                        .collect()
                }
                24 => {
                    raw.chunks_exact(3)
                        .map(|chunk| {
                            // 24-bit signed integer, sign-extend to i32
                            let sample = ((chunk[2] as i32) << 24
                                | (chunk[1] as i32) << 16
                                | (chunk[0] as i32) << 8)
                                >> 8;
                            sample as f32 / 8_388_607.0 // 2^23 - 1
                        })
                        .collect()
                }
                32 => {
                    raw.chunks_exact(4)
                        .map(|chunk| {
                            let sample = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                            sample as f32 / i32::MAX as f32
                        })
                        .collect()
                }
                _ => {
                    warn!("Unsupported bits_per_sample: {bits_per_sample}, assuming 16-bit");
                    raw.chunks_exact(2)
                        .map(|chunk| {
                            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                            sample as f32 / i16::MAX as f32
                        })
                        .collect()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_f32_samples() {
        let sample: f32 = 0.5;
        let raw = sample.to_le_bytes().to_vec();
        let result = convert_to_f32(&raw, &SampleType::Float, 32);
        assert_eq!(result.len(), 1);
        assert!((result[0] - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn convert_i16_samples() {
        let sample: i16 = i16::MAX / 2;
        let raw = sample.to_le_bytes().to_vec();
        let result = convert_to_f32(&raw, &SampleType::Int, 16);
        assert_eq!(result.len(), 1);
        assert!((result[0] - 0.5).abs() < 0.01);
    }

    #[test]
    fn convert_i32_samples() {
        let sample: i32 = i32::MAX / 2;
        let raw = sample.to_le_bytes().to_vec();
        let result = convert_to_f32(&raw, &SampleType::Int, 32);
        assert_eq!(result.len(), 1);
        assert!((result[0] - 0.5).abs() < 0.01);
    }

    #[test]
    fn convert_i24_samples() {
        // 24-bit max positive = 8388607 (0x7FFFFF)
        let raw = vec![0xFF, 0xFF, 0x7F]; // little-endian 0x7FFFFF
        let result = convert_to_f32(&raw, &SampleType::Int, 24);
        assert_eq!(result.len(), 1);
        assert!((result[0] - 1.0).abs() < 0.01);
    }

    #[test]
    fn silence_injection_produces_correct_buffers() {
        let clock = SyncClock::new();
        let buffers = Arc::new(Mutex::new(VecDeque::new()));
        inject_silence(&buffers, &clock, 2, 48000);
        let bufs = buffers.lock().unwrap();
        assert_eq!(bufs.len(), SILENCE_BUFFERS_ON_SWITCH);
        for buf in bufs.iter() {
            assert_eq!(buf.channels, 2);
            assert_eq!(buf.sample_rate, 48000);
            assert!(buf.samples.iter().all(|&s| s == 0.0));
        }
    }

    #[test]
    fn device_check_interval_is_reasonable() {
        // At 10ms poll interval, should check every ~2 seconds
        let check_period_ms = DEVICE_CHECK_INTERVAL * 10;
        assert!(check_period_ms >= 1000, "Should check at least every 1s");
        assert!(check_period_ms <= 5000, "Should check at most every 5s");
    }
}
