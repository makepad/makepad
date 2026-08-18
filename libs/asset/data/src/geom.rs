//! Minimal geometry value types for manifests and plans.
//!
//! Deliberately not makepad-math: shared protocol types carry no engine
//! dependency, and these are storage values, not math — consumers convert at
//! their own boundary. All floats must be finite; validation refuses NaN/Inf
//! and the codec refuses the non-canonical `-0.0` bit pattern on decode.

use crate::codec::{CanonReader, CanonWriter};
use crate::error::AssetDataError;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    pub const ONE: Self = Self {
        x: 1.0,
        y: 1.0,
        z: 1.0,
    };

    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub(crate) fn validate(&self, what: &'static str) -> Result<(), AssetDataError> {
        for v in [self.x, self.y, self.z] {
            if !v.is_finite() {
                return Err(AssetDataError::Malformed { what });
            }
        }
        Ok(())
    }
    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        w.f32(self.x);
        w.f32(self.y);
        w.f32(self.z);
    }
    pub(crate) fn decode(r: &mut CanonReader, what: &'static str) -> Result<Self, AssetDataError> {
        Ok(Self {
            x: r.f32(what)?,
            y: r.f32(what)?,
            z: r.f32(what)?,
        })
    }
}

/// Unit quaternion rotation, `x,y,z,w`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    pub(crate) fn validate(&self, what: &'static str) -> Result<(), AssetDataError> {
        let mut sq = 0.0f32;
        for v in [self.x, self.y, self.z, self.w] {
            if !v.is_finite() {
                return Err(AssetDataError::Malformed { what });
            }
            sq += v * v;
        }
        // Authored rotations must be unit within tolerance; a wildly scaled
        // quaternion is authoring corruption, not a style choice.
        if (sq - 1.0).abs() > 1e-3 {
            return Err(AssetDataError::Malformed { what });
        }
        Ok(())
    }
    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        w.f32(self.x);
        w.f32(self.y);
        w.f32(self.z);
        w.f32(self.w);
    }
    pub(crate) fn decode(r: &mut CanonReader, what: &'static str) -> Result<Self, AssetDataError> {
        Ok(Self {
            x: r.f32(what)?,
            y: r.f32(what)?,
            z: r.f32(what)?,
            w: r.f32(what)?,
        })
    }
}

/// Authored placement: position, rotation, per-axis scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        pos: Vec3::ZERO,
        rot: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub(crate) fn validate(&self, what: &'static str) -> Result<(), AssetDataError> {
        self.pos.validate(what)?;
        self.rot.validate(what)?;
        self.scale.validate(what)?;
        for v in [self.scale.x, self.scale.y, self.scale.z] {
            if v <= 0.0 {
                return Err(AssetDataError::Malformed { what });
            }
        }
        Ok(())
    }
    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        self.pos.encode(w);
        self.rot.encode(w);
        self.scale.encode(w);
    }
    pub(crate) fn decode(r: &mut CanonReader, what: &'static str) -> Result<Self, AssetDataError> {
        Ok(Self {
            pos: Vec3::decode(r, what)?,
            rot: Quat::decode(r, what)?,
            scale: Vec3::decode(r, what)?,
        })
    }
}

/// Axis-aligned bounds in the asset's declared coordinate system.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Bounds {
    pub(crate) fn validate(&self, what: &'static str) -> Result<(), AssetDataError> {
        self.min.validate(what)?;
        self.max.validate(what)?;
        if self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z {
            return Err(AssetDataError::Malformed { what });
        }
        Ok(())
    }
    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        self.min.encode(w);
        self.max.encode(w);
    }
    pub(crate) fn decode(r: &mut CanonReader, what: &'static str) -> Result<Self, AssetDataError> {
        Ok(Self {
            min: Vec3::decode(r, what)?,
            max: Vec3::decode(r, what)?,
        })
    }
}
