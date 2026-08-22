use crate::error::Result;

/// Fine-grained progress hook threaded through long pipeline phases:
/// `(label, phase-local fraction 0..=1)`. Returning `Err` (typically
/// [`crate::DiffusionError::Cancelled`]) unwinds the run at the next emission
/// boundary. Emission sites throttle themselves (per layer/block, or per
/// 256MB of weight bytes) so the hook never sits on a hot path.
pub type ProgressHook<'a> = &'a mut dyn FnMut(&str, f64) -> Result<()>;

/// Emits through an optional [`ProgressHook`]; no-op when absent.
pub fn emit_progress(
    progress: &mut Option<ProgressHook>,
    label: &str,
    fraction: f64,
) -> Result<()> {
    match progress {
        Some(hook) => hook(label, fraction),
        None => Ok(()),
    }
}

/// Owned form of [`ProgressHook`] for band-remapping wrappers.
pub type BoxedProgressHook<'a> = Box<dyn FnMut(&str, f64) -> Result<()> + 'a>;

/// Remaps a callee's phase-local 0..=1 fraction onto the `[base, base + span]`
/// band of the caller's own progress scale, forwarding labels as-is. Returns
/// None (a free no-op for the callee) when the hook is absent.
pub fn band_progress<'a>(
    progress: &'a mut Option<ProgressHook<'_>>,
    base: f64,
    span: f64,
) -> Option<BoxedProgressHook<'a>> {
    progress.as_mut().map(|hook| -> BoxedProgressHook<'a> {
        Box::new(move |label: &str, fraction: f64| {
            hook(label, base + fraction.clamp(0.0, 1.0) * span)
        })
    })
}

/// Reborrows a boxed band hook as the `Option<ProgressHook>` callees take.
pub fn hook_ref<'a>(sub: &'a mut Option<BoxedProgressHook<'_>>) -> Option<ProgressHook<'a>> {
    sub.as_mut().map(|hook| &mut **hook as ProgressHook)
}

/// Weight-streaming loops emit at most once per this many cumulative bytes.
pub const BYTE_PROGRESS_STEP: usize = 256 * 1024 * 1024;

/// Emits "`phase` 3.2/9.5GB" with fraction `done/total`; no-op when the hook
/// is absent (the label is not even formatted).
pub fn emit_byte_progress(
    progress: &mut Option<ProgressHook>,
    phase: &str,
    done: usize,
    total: usize,
) -> Result<()> {
    if progress.is_none() {
        return Ok(());
    }
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let fraction = if total == 0 {
        1.0
    } else {
        done as f64 / total as f64
    };
    emit_progress(
        progress,
        &format!("{phase} {:.1}/{:.1}GB", done as f64 / GB, total as f64 / GB),
        fraction,
    )
}
