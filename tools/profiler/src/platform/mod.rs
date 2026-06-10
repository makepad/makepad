use crate::{CaptureConfig, CaptureSink, ProfilerError};

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub fn capture_while<F>(
    config: &CaptureConfig,
    sink: &mut dyn CaptureSink,
    should_continue: F,
) -> Result<(), ProfilerError>
where
    F: FnMut() -> Result<bool, ProfilerError>,
{
    macos::capture_while(config, sink, should_continue)
}

#[cfg(not(target_os = "macos"))]
pub fn capture_while<F>(
    _config: &CaptureConfig,
    _sink: &mut dyn CaptureSink,
    _should_continue: F,
) -> Result<(), ProfilerError>
where
    F: FnMut() -> Result<bool, ProfilerError>,
{
    Err(ProfilerError::new(
        "makepad-profiler currently supports macOS only",
    ))
}
