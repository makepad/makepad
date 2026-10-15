//! A headset display's optics, independent of the device that reports them.
//!
//! A display backend (the Valve board's factory config today) fills a
//! `DisplayCalibration`. The VR view applies it only when it draws to that
//! headset's own screen; the side-by-side preview on an ordinary display
//! keeps the uncalibrated defaults, because these optics describe the
//! headset's lenses and panels, not a monitor.
//!
//! Conventions (the renderer's, matching OpenXR view poses):
//! - Head frame: x right, y up, z toward the viewer.
//! - `eye_to_head` maps an eye-frame vector to the head frame
//!   (`v_head = M · v_eye`), so its columns are the eye's axes in the head
//!   frame. A canted display has an eye forward (`-M[..][2]`) turned away
//!   from the head's -z.
//! - The field of view is four tangents measured from the eye's -z:
//!   `left`/`down` negative, `right`/`up` positive.
//! - Eye images are rendered with v DOWN (top row = `up`). A lens model maps
//!   a display position (u right, v down, both 0..1 over that eye's panel)
//!   to a position in the rendered eye image, per colour channel.

/// The radial function of a Valve lighthouse distortion block, as Valve
/// documents it ("The JSON File (Lighthouse Devices)", OpenVR wiki):
/// `R · D(R / (w/2)) = f · tan θ` with
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValveRadial {
    /// `DISTORT_POLY3`: `D(x) = 1 + C0·x² + C1·x⁴ + C2·x⁶`.
    Poly3,
    /// `DISTORT_DPOLY3`: `D(x) = 1 / (1 + C0·x² + C1·x⁴ + C2·x⁶)`.
    DPoly3,
}

/// How a display position finds its source in the rendered eye image.
#[derive(Clone, Debug, PartialEq)]
pub enum LensModel {
    /// The image as rendered: no correction.
    Identity,
    /// Valve's lighthouse-config distortion, per channel R, G, B. The
    /// mapping is Monado's `u_compute_distortion_vive`
    /// (src/xrt/auxiliary/util/u_distortion_mesh.c, main ec188bb1), which
    /// follows from Valve's definitions of `intrinsics`, `center_x/y`
    /// (normalised by w/2) and `grow_for_undistort`:
    ///
    /// ```text
    /// f   = 0.5 / (1 + grow);  factor = (f, f · aspect)
    /// t   = (2u − 1, (2v − 1) / aspect) − center[c]
    /// x²  = t·t
    /// d   = D_c(x)        (Poly3 or DPoly3 per channel, Valve's types)
    /// tc  = 0.5 + (t·d + center[c]) · factor
    /// ```
    ///
    /// Monado always uses the DPoly3 form; Valve's `type` field says which
    /// applies, and this headset's config says `DISTORT_POLY3`.
    ValveRadial {
        radial: [ValveRadial; 3],
        center: [[f32; 2]; 3],
        /// C0, C1, C2 per channel.
        k: [[f32; 3]; 3],
        grow_for_undistort: f32,
        /// The eye panel's width over height: `physical_aspect_x_over_y`,
        /// else the eye target's pixel width over height (square pixels).
        aspect_x_over_y: f32,
    },
}

/// One eye's optics.
#[derive(Clone, Debug, PartialEq)]
pub struct EyeCalibration {
    /// Rows of the eye-to-head rotation (canting).
    pub eye_to_head: [[f32; 3]; 3],
    /// Tangents of the rendered field of view: left, right, up, down.
    pub fov_tan: [f32; 4],
    pub lens: LensModel,
}

/// A headset display's optics, as its backend reported them.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayCalibration {
    /// Where the numbers came from (device, serial), for logs.
    pub source: String,
    /// The display's EDID manufacturer letters, when the backend knows them,
    /// so the VR view applies the optics only to that headset's screen.
    pub edid_vendor: Option<[u8; 3]>,
    /// The distance between the lens centres, metres: the eye separation a
    /// fixed-IPD headset is built for. An explicit `ipd_mm` overrides it.
    pub lens_separation: Option<f32>,
    /// Device-reported seconds from vsync to panel illumination. Kept in
    /// the calibration so every tracker backend can supply its own timing.
    pub vsync_to_photons: Option<f64>,
    /// Left, right.
    pub eyes: [EyeCalibration; 2],
}

impl EyeCalibration {
    /// The projection uniform for the renderer: x scale, x offset, y scale,
    /// y offset, so that clip x = p.x·sx + ox·w and clip y = p.y·sy + oy·w
    /// with w = −p.z (an off-axis frustum through the four tangents).
    pub fn projection(&self) -> [f32; 4] {
        let [l, r, u, d] = self.fov_tan;
        let (w, h) = ((r - l).max(1e-4), (u - d).max(1e-4));
        [2.0 / w, -(r + l) / w, 2.0 / h, -(u + d) / h]
    }
}
