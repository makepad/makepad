//! The environment's share of the frame's sun rig (HDRI phase 2, C5/C6):
//! the prepared IBL's SH9, mean luminance and horizon band read into a
//! `sun::EnvLighting` and applied at the sun sites in frame.rs and
//! bake_passes.rs. The arithmetic lives in sun.rs, Cx-free, so
//! tests/one_sun.rs pins it without a device; this file only knows where
//! the renderer keeps the preparation (renderer/ibl.rs).
//!
//! Every function here is keyed on the WORLD's own environment first (an
//! IBL named and finite), and only then reads the bound preparation: a draw
//! whose world names none (an aux draw's default environment, a host's
//! analytic world) gets exactly the rig it had before, whatever the scene
//! last bound.
use super::*;
use crate::sun::{EnvLighting, SunLight};

impl Renderer {
    /// What the environment lends the rig this frame: `Some` once the world
    /// names a finite IBL and a preparation has landed. renderer/ibl.rs
    /// keeps the previous preparation while a new one runs, so this can be
    /// one preparation behind the world — and so is the dome, which is the
    /// point: light and sky always come from the same preparation. The key
    /// light is the bound preparation's own (`ibl_sun`: the sun the world
    /// declared when it was submitted, else a procedural hdri preset's
    /// baked sun), never the key the world declares meanwhile: a key lit
    /// before its preparation lands would be lit twice (its light is still
    /// in the bound lighting copy) and metered against the wrong map. In
    /// the map's frame; `sun::env_sun_dir_with` turns it by
    /// `Ibl.rotation_deg`.
    ///
    /// The meter. renderer/ibl.rs meters the LIGHTING copy (the map without
    /// its key: exactly for a generated map, the covering cone filled for a
    /// file's sun), so the key's share of the sphere mean is added
    /// back here: its emission `L·2π(1 − cos r)` (`EnvSun`'s meaning: L is
    /// the cone's average) over the sphere's 4π, times the share a surface
    /// facing it receives (`facing`; the same product the directional light's
    /// colour has, `sun::env_sun_scale`, so the meter counts what the light
    /// delivers, as the stock meter does). The stock meter
    /// (`SunLight::hdr_exposure`) counts the sun too; without the share a
    /// sunny map would expose well over.
    pub(super) fn env_lighting(&self, world: &World) -> Option<EnvLighting> {
        let ibl = world.environment.ibl.filter(|i| i.intensity.is_finite() && i.rotation_deg.is_finite())?;
        let sh = *self.ibl_sh9()?;
        let mean_luminance = self.ibl_mean_luminance()?;
        let sun_share = self.ibl_sun().map_or(0.0, |s| {
            // L·2(1 − cos r)·facing / 4 = L·(1 − cos r)/2·facing.
            crate::sky::luminance(s.radiance) * crate::sun::env_sun_scale(&s) * 0.25
        });
        let sun_share = if sun_share.is_finite() { sun_share.max(0.0) } else { 0.0 };
        // The environment lights at its own scale × Ibl.intensity, exactly
        // what the dome and mat_ibl_* show (HDR_SKY_GAIN is the analytic
        // sky's alone).
        Some(EnvLighting { sh, mean_luminance: mean_luminance + sun_share, gain: ibl.intensity.max(0.0) })
    }

    /// The environment's sun direction for the systems that read the
    /// direction before the rig takes its colours (streams, cascades, the
    /// bake): `None` without a prepared environment, so those paths stay
    /// exactly what they were.
    pub(super) fn env_sun_dir(&self, world: &World) -> Option<Vec3f> {
        self.env_lighting(world)?;
        crate::sun::env_sun_dir_with(world, self.ibl_sun())
    }

    /// `sun::env_sun_rig` on this renderer's lane, with the bound
    /// preparation's key.
    pub(super) fn env_sun_rig(&self, world: &World, sun: SunLight) -> SunLight {
        crate::sun::env_sun_rig_with(world, self.ibl_sun(), self.env_lighting(world).as_ref(), sun, self.hdr_output)
    }

    /// The bound map's own sun in world space for the daylight switches:
    /// `None` without a prepared environment or when the map knows no sun of
    /// its own (renderer/ibl.rs `ibl_daylight_sun`).
    pub(super) fn env_daylight_dir(&self, world: &World) -> Option<Vec3f> {
        self.env_lighting(world)?;
        crate::sun::env_daylight_dir_with(world, self.ibl_daylight_sun())
    }

    /// N1: the direction the frame's "is it day?" switches read (the street
    /// lamps' photocell, a streamed city's night factor with its lit windows
    /// and headlights, and the analytic sky unless the light is the sun:
    /// `frame_sky`): a world's own Sun, else the environment's own sun (the
    /// bound map's, even while the moon or no key at all lights the frame),
    /// else the world's own sun (`resolve_sun`):
    /// a map that knows no sun of its own (a studio, a loaded file) leaves
    /// the switches there, as without an environment. Never the key's
    /// direction: a moon key lights the frame and casts its shadows, but a
    /// moonlit night is still night. Without an environment this is the
    /// direction the frame's rig starts from, bit for bit.
    pub(super) fn daylight_dir(&self, world: &World) -> Vec3f {
        crate::world_lights::world_sun_dir(world)
            .or_else(|| self.env_daylight_dir(world))
            .unwrap_or_else(|| crate::sun::resolve_sun(&world.sun).dir)
    }

    /// The bound preparation's key is its map's own sun
    /// (`sun::env_key_is_its_sun`), for a world that names a prepared
    /// environment: the light the frame aims is then the sun the sky shows.
    pub(super) fn env_key_is_its_sun(&self, world: &World) -> bool {
        self.env_lighting(world).is_some() && crate::sun::env_key_is_its_sun(self.ibl_sun(), self.ibl_daylight_sun())
    }

    /// I1c: the rig the frame's lamps are railed against: the frame's own,
    /// the environment's sun, fill and exposure folded in
    /// (`sun::env_sun_rig_with`), in the display units the rails measure in
    /// whatever the lane (`lamp_daylight_scale`'s headroom under white). In
    /// the legacy lane it is the rig the bake snapshots too, so a lamp's
    /// per-frame term and its baked pool are railed alike (lights.rs: they
    /// "may never disagree"); in the clustered HDR lane only the photocell
    /// switches the lamps, and it reads `daylight_dir`. Without an
    /// environment it is `sun` bit for bit.
    pub(super) fn env_lamp_rig(&self, world: &World, sun: SunLight) -> SunLight {
        crate::sun::env_sun_rig_with(world, self.ibl_sun(), self.env_lighting(world).as_ref(), sun, false)
    }

    /// C6: the fog colour the environment lends a `Fog::Host` world whose
    /// background is the environment. `None` in an MR stage (the room
    /// supplies the horizon), for an authored `Fog` (world_lights::world_fog
    /// already answered), without a prepared environment, and (K4) under any
    /// other background: the fog is the dome's horizon, so where the host's
    /// sky (or a colour) is drawn, the host's fog meets it, whatever the
    /// environment lights with. The density stays the host's
    /// (`SkyConfig::fog`).
    ///
    /// The colour is the dome's own at the horizon, at the environment's
    /// own scale × Ibl.intensity (no HDR_SKY_GAIN: that is the analytic
    /// sky's). HDR lane: the band, linear × the gain. Legacy lane: the band
    /// through the dome's tone map at the dome's exposure (renderer/ibl.rs
    /// `env_dome_controls`, fed the same metered luminance, the map's mean
    /// × Ibl.intensity, that `draw_environment_background` passes), so the
    /// haze meets the sky drawn behind it.
    pub(super) fn env_fog_color(&self, world: &World, shows_environment: bool) -> Option<Vec3f> {
        if !shows_environment || !matches!(world.environment.fog, makepad_scene::Fog::Host) {
            return None;
        }
        // The dome is drawn at the background's own intensity too.
        let makepad_scene::Background::Environment { intensity, .. } = world.environment.background else { return None };
        let background = if intensity.is_finite() { intensity.max(0.0) } else { 1.0 };
        let env = self.env_lighting(world)?;
        let horizon = self.ibl_horizon_rgb()? * background;
        if self.hdr_output {
            return Some(crate::sun::env_fog_color(horizon, &env));
        }
        let scale = env.gain; // Ibl.intensity, finite and >= 0 (env_lighting)
        let ev = world.sky.as_ref().map(|s| s.exposure_ev).filter(|e| e.is_finite()).unwrap_or(0.0);
        let (_, bg2) = super::ibl::env_dome_controls(0.0, 1.0, &Mat4f::identity(), false, self.ibl_mean_luminance()? * scale, ev, None);
        Some(crate::sun::legacy_dome_rgb(horizon * scale, bg2.x))
    }

    /// A preparation job is running (renderer/ibl.rs). The brief's seam for
    /// C8's renderer tests; nothing in a non-test build reads it yet.
    #[allow(dead_code)]
    pub(super) fn ibl_pending(&self) -> bool {
        self.environment_pending()
    }

    /// How many preparations this renderer has started (tests: C8).
    #[allow(dead_code)]
    pub(super) fn ibl_preparations(&self) -> u64 {
        self.environment_preparations()
    }
}
