use super::{AudioBuffer, AudioCapture, AudioConfig, AudioError};
use crate::sync::clock::SyncClock;
use log::error;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode};

/// Windows audio capture using WASAPI loopback mode.
///
/// Captures system audio output (what the user hears) by opening
/// the default render device in loopback mode. Uses polling (not
/// event-driven) because WASAPI loopback doesn't support event mode.
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

/// The WASAPI capture loop running on a dedicated thread.
fn wasapi_capture_loop(
    running: Arc<Mutex<bool>>,
    buffers: Arc<Mutex<VecDeque<AudioBuffer>>>,
    clock: SyncClock,
    _config: AudioConfig,
) -> Result<(), AudioError> {
    // Initialize COM for this thread (returns HRESULT, use .ok() to get Result)
    wasapi::initialize_mta()
        .ok()
        .map_err(|e| AudioError::Platform(format!("COM init failed: {e}")))?;

    // Get default render (output) device for loopback
    let enumerator = DeviceEnumerator::new()
        .map_err(|e| AudioError::Platform(format!("Failed to create enumerator: {e}")))?;
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
    let sample_type = mix_format
        .get_subformat()
        .map_err(|e| AudioError::Platform(format!("Failed to get sample type: {e}")))?;

    // Initialize in shared polling mode for loopback capture.
    // Direction::Render + loopback captures what's being played on the device.
    let mode = StreamMode::PollingShared {
        autoconvert: true,
        buffer_duration_hns: 0, // default buffer
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

    let poll_interval = Duration::from_millis(10);
    let mut sample_queue: VecDeque<u8> = VecDeque::new();

    loop {
        {
            let r = running.lock().unwrap_or_else(|e| e.into_inner());
            if !*r {
                break;
            }
        }

        // Read available data into the queue
        match capture_client.read_from_device_to_deque(&mut sample_queue) {
            Ok(_) => {
                // Process all complete samples from the queue
                if !sample_queue.is_empty() {
                    let raw_bytes: Vec<u8> = sample_queue.drain(..).collect();
                    let samples = convert_to_f32(&raw_bytes, &sample_type, channels);
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
fn convert_to_f32(raw: &[u8], sample_type: &SampleType, _channels: u16) -> Vec<f32> {
    match sample_type {
        SampleType::Float => {
            raw.chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect()
        }
        SampleType::Int => {
            // WASAPI Int format is typically 16-bit or 32-bit.
            // The mix format's bits_per_sample tells us which, but since we
            // only have the raw bytes, try 16-bit (most common for Int).
            raw.chunks_exact(2)
                .map(|chunk| {
                    let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                    sample as f32 / i16::MAX as f32
                })
                .collect()
        }
    }
}
