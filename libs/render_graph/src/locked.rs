//! Locked time's history inventory (KERNELS.md §3.4.3).
//!
//! [`RenderMode::LockedTime`](crate::RenderMode) is valid only when every
//! stateful subsystem of the frame has a rule. A renderer reports what each
//! of its subsystems is doing this frame as a [`Usage`]; [`check`] applies
//! the table below and refuses the frame with a diagnostic naming the first
//! subsystem that has no locked-time rule for what it is doing. It never
//! renders approximately.
//!
//! | Subsystem | Locked-time rule |
//! |---|---|
//! | auto-exposure | fixed or metered (analytic at `t`); adaptation refused |
//! | sun and day cycle | analytic at `t`; eased values refused |
//! | render-scale governor | a fixed scale; the governor refused |
//! | lightmap bakes | baked to completion before the frame, or refused |
//! | SSAO | current frame (a depth pre-pass), or off; previous-frame depth refused |
//! | fast GI | off until its cold-converge mode exists |
//! | VFX particles | a closed form of (spawn time, index, seed) or the stepper; free-running refused |
//! | TAA | refused (supersampling instead) |
//! | occluder dither, FXAA | allowed (no history) |
//! | kernels | recomputed at the canonical time, synchronously |
//! | stepper | seeked to the canonical time |
//! | resource readiness | every pipeline, asset, kernel and sim output ready; a fallback pipeline is not ready |
//! | VJ feedback, hold, GPU sims, frame scripts | refused |

/// The stateful subsystems a frame can depend on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Subsystem {
    AutoExposure,
    SunCycle,
    RenderScale,
    LightmapBake,
    Ssao,
    FastGi,
    VfxParticles,
    Taa,
    OccluderDither,
    Fxaa,
    Kernels,
    Stepper,
    Readiness,
    VjHistory,
}

impl Subsystem {
    pub fn name(self) -> &'static str {
        match self {
            Self::AutoExposure => "auto-exposure",
            Self::SunCycle => "sun and day cycle",
            Self::RenderScale => "render-scale governor",
            Self::LightmapBake => "lightmap bake",
            Self::Ssao => "SSAO",
            Self::FastGi => "fast GI",
            Self::VfxParticles => "VFX particles",
            Self::Taa => "TAA",
            Self::OccluderDither => "occluder dither",
            Self::Fxaa => "FXAA",
            Self::Kernels => "kernels",
            Self::Stepper => "effect stepper",
            Self::Readiness => "resource readiness",
            Self::VjHistory => "VJ history (feedback, hold, GPU sims, frame scripts)",
        }
    }
}

/// What a subsystem is doing this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    /// Not in use.
    Off,
    /// Evaluated from the canonical time alone (a closed form, a fixed
    /// value, a metered exposure, a fixed render scale).
    Analytic,
    /// Needs no history (camera-relative dither, FXAA).
    Stateless,
    /// Reads or integrates state carried from earlier frames (adaptation,
    /// easing, previous-frame buffers, free-running emitters, TAA history).
    History,
    /// Recomputed synchronously at the canonical time (kernels, a stepper
    /// seeked to `t`, a bake run to completion before the frame).
    Synchronous,
    /// Work still spread across frames or pending (an unfinished bake, a
    /// pipeline, asset or kernel output not ready, a fallback shader).
    Pending,
}

/// Why a locked-time frame was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub subsystem: Subsystem,
    pub usage: Usage,
    pub message: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The rule for one subsystem in one usage: `Ok` or the fix to name.
fn rule(s: Subsystem, u: Usage) -> Result<(), &'static str> {
    use Subsystem::*;
    use Usage::*;
    match (s, u) {
        (_, Off) => Ok(()),
        (_, Pending) => Err("is not ready: a locked-time frame waits until every pipeline, asset, bake, kernel and sim output it needs is complete"),
        (AutoExposure, Analytic) => Ok(()),
        (AutoExposure, _) => Err("adapts over frames: use a fixed or metered exposure"),
        (SunCycle, Analytic) => Ok(()),
        (SunCycle, _) => Err("eases over frames: evaluate the sun analytically at the frame time"),
        (RenderScale, Analytic) => Ok(()),
        (RenderScale, _) => Err("follows GPU timings: use a fixed render scale"),
        (LightmapBake, Analytic | Synchronous) => Ok(()),
        (LightmapBake, _) => Err("spreads its bake over frames: bake to completion before the frame"),
        (Ssao, Synchronous | Analytic) => Ok(()),
        (Ssao, _) => Err("reads the previous frame's depth: use current-frame AO (a depth pre-pass) or turn it off"),
        (FastGi, _) => Err("carries last frame's field and a timing-dependent budget: turn GI off for locked time (its cold-converge mode is not built)"),
        (VfxParticles, Analytic | Synchronous) => Ok(()),
        (VfxParticles, _) => Err("free-running emitters are realtime only: use a closed-form (spawn time, index, seed) kernel or the effect stepper"),
        (Taa, _) => Err("keeps a history buffer: use supersampling (Aa::Ssaa) in locked time"),
        (OccluderDither | Fxaa, Stateless | Analytic) => Ok(()),
        (OccluderDither | Fxaa, _) => Err("must not carry history"),
        (Kernels | Stepper, Synchronous | Analytic) => Ok(()),
        (Kernels, _) => Err("results must be recomputed at the canonical time, synchronously"),
        (Stepper, _) => Err("must be seeked to the canonical time"),
        (Readiness, Analytic | Synchronous | Stateless) => Ok(()),
        (Readiness, History) => Err("is showing a result from an earlier frame"),
        (VjHistory, _) => Err("keeps arbitrary history and cannot be seeked"),
    }
}

/// Check a frame's subsystems for locked time: `Ok` when every one has a
/// rule for what it is doing, else the first refusal (in the order given).
pub fn check(usages: &[(Subsystem, Usage)]) -> Result<(), Refusal> {
    for &(subsystem, usage) in usages {
        if let Err(why) = rule(subsystem, usage) {
            return Err(Refusal { subsystem, usage, message: format!("locked time refused: {} {why}", subsystem.name()) });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_refused_with_its_fix() {
        let err = check(&[(Subsystem::Fxaa, Usage::Stateless), (Subsystem::AutoExposure, Usage::History)]).unwrap_err();
        assert_eq!(err.subsystem, Subsystem::AutoExposure);
        assert!(err.message.contains("fixed or metered"), "{}", err.message);
        assert!(check(&[(Subsystem::Taa, Usage::History)]).is_err());
        assert!(check(&[(Subsystem::Ssao, Usage::History)]).unwrap_err().message.contains("pre-pass"));
    }

    #[test]
    fn fast_gi_is_refused_until_cold_converge_exists() {
        assert!(check(&[(Subsystem::FastGi, Usage::Off)]).is_ok());
        for u in [Usage::Analytic, Usage::Synchronous, Usage::History] {
            assert!(check(&[(Subsystem::FastGi, u)]).is_err());
        }
    }

    #[test]
    fn a_pending_resource_is_never_whole() {
        let err = check(&[(Subsystem::Readiness, Usage::Pending)]).unwrap_err();
        assert!(err.message.contains("not ready"));
        assert!(check(&[(Subsystem::LightmapBake, Usage::Pending)]).is_err());
    }

    #[test]
    fn analytic_and_synchronous_frames_pass() {
        let frame = [
            (Subsystem::AutoExposure, Usage::Analytic),
            (Subsystem::SunCycle, Usage::Analytic),
            (Subsystem::RenderScale, Usage::Analytic),
            (Subsystem::LightmapBake, Usage::Synchronous),
            (Subsystem::Ssao, Usage::Off),
            (Subsystem::FastGi, Usage::Off),
            (Subsystem::VfxParticles, Usage::Off),
            (Subsystem::Taa, Usage::Off),
            (Subsystem::OccluderDither, Usage::Stateless),
            (Subsystem::Fxaa, Usage::Stateless),
            (Subsystem::Kernels, Usage::Synchronous),
            (Subsystem::Stepper, Usage::Off),
            (Subsystem::Readiness, Usage::Synchronous),
            (Subsystem::VjHistory, Usage::Off),
        ];
        assert_eq!(check(&frame), Ok(()));
    }
}
