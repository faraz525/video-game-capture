use super::{AudioBuffer, AudioCapture, AudioConfig, AudioError};
use crate::sync::clock::SyncClock;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use wasapi::{AudioClient, Device, Direction, SampleType, ShareMode};

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
                eprintln!("[GameClip] WASAPI capture error: {e}");
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
    config: AudioConfig,
) -> Result<(), AudioError> {
    // Initialize COM for this thread
    wasapi::initialize_mta()
        .map_err(|e| AudioError::Platform(format!("COM init failed: {e}")))?;

    // Get default render (output) device for loopback
    let device = Device::new_default_device(Direction::Render)
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
        .get_sampletype()
        .map_err(|e| AudioError::Platform(format!("Failed to get sample type: {e}")))?;

    // Initialize in shared mode with loopback
    audio_client
        .initialize_client(
            &mix_format,
            0,      // period (0 = default)
            &Direction::Capture,
            &ShareMode::Shared,
            true,   // loopback = true
        )
        .map_err(|e| AudioError::Platform(format!("Failed to init audio client: {e}")))?;

    let capture_client = audio_client
        .get_audiocaptureclient()
        .map_err(|e| AudioError::Platform(format!("Failed to get capture client: {e}")))?;

    audio_client
        .start_stream()
        .map_err(|e| AudioError::Platform(format!("Failed to start stream: {e}")))?;

    let poll_interval = Duration::from_millis(10);

    loop {
        {
            let r = running.lock().unwrap_or_else(|e| e.into_inner());
            if !*r {
                break;
            }
        }

        // Poll for available data
        match capture_client.get_next_nbr_frames() {
            Ok(Some(n_frames)) if n_frames > 0 => {
                match capture_client.read_from_device(n_frames as usize) {
                    Ok(raw_bytes) => {
                        let samples = convert_to_f32(&raw_bytes, &sample_type, channels);
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
                    Err(e) => {
                        eprintln!("[GameClip] WASAPI read error: {e}");
                    }
                }
            }
            Ok(_) => {
                // No frames available, wait and retry
            }
            Err(e) => {
                eprintln!("[GameClip] WASAPI poll error: {e}");
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
        SampleType::Int16 => {
            raw.chunks_exact(2)
                .map(|chunk| {
                    let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                    sample as f32 / i16::MAX as f32
                })
                .collect()
        }
        SampleType::Int32 => {
            raw.chunks_exact(4)
                .map(|chunk| {
                    let sample = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    sample as f32 / i32::MAX as f32
                })
                .collect()
        }
    }
}
