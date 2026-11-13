//! Graphics quality tiers: what a scene spends on its look, from an
//! integrated laptop GPU to a desktop card.
//!
//! A tier sets the static budgets that cost GPU time in an outdoor scene:
//! where trees turn into impostors (a mesh tree costs far more than its
//! stand-in), how thickly the grass grows on screen, the sun cascades'
//! resolution and reach, screen-space occlusion and the render scale cap.
//! The adaptive governor (thermometer.rs) still trims decoration on top of
//! the tier when the frame time runs over; the tier is where it starts.
//!
//! Auto picks the tier once from the GPU the backend reports
//! ([`GraphicsTier::for_gpu`]); the host shows the choice as a direct
//! control and the user's pick overrides it.

/// One of the quality tiers, cheapest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GraphicsTier {
    Low,
    Medium,
    High,
    Ultra,
}

/// What a tier lets a scene spend.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TierBudget {
    /// Multiplier on every model's impostor distance.
    pub impostor_scale: f32,
    /// The narrowest a far grass blade is drawn, in screen pixels: every
    /// blade grows out to where one is this wide, then they thin and widen
    /// (crate::grass::GrassLod), so the blade count follows the image.
    pub grass_pixels: f32,
    /// One sun cascade's edge, texels, and the detail range, metres.
    pub csm_resolution: usize,
    pub csm_range: f32,
    /// Screen-space ambient occlusion.
    pub ssao: bool,
    /// The largest render scale the host's automatic scale may use.
    pub render_scale: f32,
    /// How far a level terrain tile blends its paint layers (the splat
    /// lane's per-pixel layer walk, a texture read per candidate layer);
    /// beyond it the heaviest layer alone.
    pub terrain_blend_reach: f32,
    /// The trampled-grass field around the focus (crate::trample): its side
    /// in metres and texels; None: the blades are never pressed.
    pub trample: Option<crate::trample::TrampleBudget>,
}

impl GraphicsTier {
    pub const ALL: [GraphicsTier; 4] = [GraphicsTier::Low, GraphicsTier::Medium, GraphicsTier::High, GraphicsTier::Ultra];

    pub fn label(self) -> &'static str {
        match self {
            GraphicsTier::Low => "Low",
            GraphicsTier::Medium => "Medium",
            GraphicsTier::High => "High",
            GraphicsTier::Ultra => "Ultra",
        }
    }

    pub fn budget(self) -> TierBudget {
        match self {
            GraphicsTier::Low => TierBudget { impostor_scale: 0.8, grass_pixels: 6.0, csm_resolution: 1024, csm_range: 40.0, ssao: false, render_scale: 0.75, terrain_blend_reach: 150.0, trample: None },
            GraphicsTier::Medium => TierBudget { impostor_scale: 1.25, grass_pixels: 4.0, csm_resolution: 1536, csm_range: 60.0, ssao: false, render_scale: 1.0, terrain_blend_reach: 450.0, trample: Some(crate::trample::TrampleBudget { extent: 64.0, texels: 512 }) },
            GraphicsTier::High => TierBudget { impostor_scale: 1.6, grass_pixels: 2.5, csm_resolution: 2048, csm_range: 80.0, ssao: true, render_scale: 1.0, terrain_blend_reach: f32::MAX, trample: Some(crate::trample::TrampleBudget { extent: 96.0, texels: 1024 }) },
            GraphicsTier::Ultra => TierBudget { impostor_scale: 2.4, grass_pixels: 1.6, csm_resolution: 4096, csm_range: 120.0, ssao: true, render_scale: 1.0, terrain_blend_reach: f32::MAX, trample: Some(crate::trample::TrampleBudget { extent: 96.0, texels: 1024 }) },
        }
    }

    /// The tier Auto starts at for a GPU, from what the backend reports:
    /// phone, headset and software rasterisers Low, integrated parts
    /// Medium at most, discrete desktop cards High. The governor takes it
    /// down further when the frame time says so.
    /// `discrete`: the backend's word on the device kind (None: unknown).
    pub fn for_gpu(vendor: &str, renderer: &str, discrete: Option<bool>) -> GraphicsTier {
        let (v, r) = (vendor.to_ascii_lowercase(), renderer.to_ascii_lowercase());
        let any = |keys: &[&str]| keys.iter().any(|k| v.contains(k) || r.contains(k));
        if any(&["llvmpipe", "softpipe", "swiftshader", "software", "adreno", "mali", "powervr", "qualcomm", "videocore"]) {
            return GraphicsTier::Low;
        }
        if any(&["apple"]) {
            return if any(&["max", "ultra"]) { GraphicsTier::High } else { GraphicsTier::Medium };
        }
        match discrete {
            Some(true) => return GraphicsTier::High,
            Some(false) => return if any(&["intel"]) { GraphicsTier::Low } else { GraphicsTier::Medium },
            None => {}
        }
        if any(&["intel"]) {
            return if any(&["arc a", "arc b"]) { GraphicsTier::High } else { GraphicsTier::Low };
        }
        if any(&["nvidia", "geforce", "quadro", "rtx", "radeon rx", "radeon pro"]) {
            return GraphicsTier::High;
        }
        // An AMD APU names its graphics without a card model.
        if any(&["radeon", "amd"]) {
            return GraphicsTier::Medium;
        }
        GraphicsTier::Medium
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_starts_low_on_weak_parts_and_high_on_desktop_cards() {
        assert_eq!(GraphicsTier::for_gpu("Intel", "Intel(R) UHD Graphics 620", None), GraphicsTier::Low);
        assert_eq!(GraphicsTier::for_gpu("Mesa", "llvmpipe (LLVM 17.0.6, 256 bits)", None), GraphicsTier::Low);
        assert_eq!(GraphicsTier::for_gpu("apple", "Apple M4", Some(false)), GraphicsTier::Medium);
        assert_eq!(GraphicsTier::for_gpu("apple", "Apple M9 Max", Some(false)), GraphicsTier::High);
        assert_eq!(GraphicsTier::for_gpu("nvidia", "NVIDIA GeForce GPU-A", Some(true)), GraphicsTier::High);
        assert_eq!(GraphicsTier::for_gpu("amd", "AMD Radeon Graphics (RADV REMBRANDT)", Some(false)), GraphicsTier::Medium);
        assert_eq!(GraphicsTier::for_gpu("intel", "Intel(R) Arc(tm) A770 Graphics (DG2)", Some(true)), GraphicsTier::High);
        assert_eq!(GraphicsTier::for_gpu("intel", "Intel(R) Graphics (RPL-P)", Some(false)), GraphicsTier::Low);
    }

    #[test]
    fn tiers_spend_more_as_they_rise() {
        for w in GraphicsTier::ALL.windows(2) {
            let (a, b) = (w[0].budget(), w[1].budget());
            assert!(a.impostor_scale < b.impostor_scale && a.grass_pixels > b.grass_pixels && a.csm_resolution < b.csm_resolution, "{:?} -> {:?}", w[0], w[1]);
        }
    }
}
