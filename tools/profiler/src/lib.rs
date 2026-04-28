use makepad_micro_serde::{DeJson, DeJsonErr, DeJsonState, SerJson, SerJsonState};
use std::error::Error;
use std::fmt;
use std::time::Duration;

mod platform;

#[derive(Clone, Debug)]
pub struct CaptureConfig {
    pub process_id: u32,
    pub duration: Duration,
    pub interval: Duration,
    pub max_frames: usize,
    pub include_images: bool,
}

impl CaptureConfig {
    pub fn validate(&self) -> Result<(), ProfilerError> {
        if self.process_id == 0 {
            return Err(ProfilerError::new("process id must be non-zero"));
        }
        if self.duration.is_zero() {
            return Err(ProfilerError::new("duration must be greater than zero"));
        }
        if self.interval.is_zero() {
            return Err(ProfilerError::new("interval must be greater than zero"));
        }
        if self.max_frames == 0 {
            return Err(ProfilerError::new("max_frames must be greater than zero"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct CaptureHeader {
    pub schema: String,
    pub profiler_version: u32,
    pub platform: String,
    pub architecture: String,
    pub process_id: u32,
    pub executable: String,
    pub start_unix_micros: u64,
    pub duration_micros: u64,
    pub interval_micros: u64,
    pub max_frames: u32,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct LoadedImage {
    pub load_address: u64,
    pub file_mod_date: u64,
    pub path: String,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct ProcessSample {
    pub timestamp_micros: u64,
    pub suspend_micros: u64,
    pub threads: Vec<ThreadSample>,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct ThreadSample {
    pub thread_port: u32,
    pub thread_id: u64,
    pub run_state: u32,
    pub pc: u64,
    pub sp: u64,
    pub fp: u64,
    pub frames: Vec<u64>,
    pub complete: bool,
    pub error: String,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct Capture {
    pub header: CaptureHeader,
    pub warnings: Vec<String>,
    pub images: Vec<LoadedImage>,
    pub samples: Vec<ProcessSample>,
}

impl Capture {
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    pub fn thread_sample_count(&self) -> usize {
        self.samples.iter().map(|sample| sample.threads.len()).sum()
    }
}

pub trait CaptureSink {
    fn set_header(&mut self, header: CaptureHeader);
    fn push_warning(&mut self, warning: String);
    fn push_image(&mut self, image: LoadedImage);
    fn push_sample(&mut self, sample: ProcessSample);
}

#[derive(Default)]
pub struct CaptureCollector {
    header: Option<CaptureHeader>,
    warnings: Vec<String>,
    images: Vec<LoadedImage>,
    samples: Vec<ProcessSample>,
}

impl CaptureCollector {
    pub fn finish(mut self) -> Result<Capture, ProfilerError> {
        let Some(mut header) = self.header.take() else {
            return Err(ProfilerError::new("capture header was never emitted"));
        };
        if header.duration_micros == 0 {
            header.duration_micros = self
                .samples
                .last()
                .map(|sample| sample.timestamp_micros)
                .unwrap_or(0);
        }
        Ok(Capture {
            header,
            warnings: self.warnings,
            images: self.images,
            samples: self.samples,
        })
    }
}

impl CaptureSink for CaptureCollector {
    fn set_header(&mut self, header: CaptureHeader) {
        self.header = Some(header);
    }

    fn push_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    fn push_image(&mut self, image: LoadedImage) {
        self.images.push(image);
    }

    fn push_sample(&mut self, sample: ProcessSample) {
        self.samples.push(sample);
    }
}

pub fn capture(config: &CaptureConfig) -> Result<Capture, ProfilerError> {
    let mut collector = CaptureCollector::default();
    let started = std::time::Instant::now();
    capture_while(config, &mut collector, || {
        Ok(started.elapsed() < config.duration)
    })?;
    collector.finish()
}

pub fn capture_into(
    config: &CaptureConfig,
    sink: &mut dyn CaptureSink,
) -> Result<(), ProfilerError> {
    let started = std::time::Instant::now();
    capture_while(config, sink, || Ok(started.elapsed() < config.duration))
}

pub fn capture_while<F>(
    config: &CaptureConfig,
    sink: &mut dyn CaptureSink,
    should_continue: F,
) -> Result<(), ProfilerError>
where
    F: FnMut() -> Result<bool, ProfilerError>,
{
    config.validate()?;
    platform::capture_while(config, sink, should_continue)
}

#[derive(Clone, Debug)]
pub struct ProfilerError {
    message: String,
}

impl ProfilerError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ProfilerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for ProfilerError {}
