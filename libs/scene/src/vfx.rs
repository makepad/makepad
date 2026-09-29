//! The VFX vocabulary: effect presets as DATA, and the requests that spawn
//! them. Shared between the script layer (which builds or names presets) and
//! the renderer (which simulates and draws them). Like the particle requests
//! these are tier-3 Local: nothing here touches the world or its RNG.
//!
//! A preset is a list of particle [`VfxLayer`]s plus an optional light flash
//! and an optional surface mark. The built-in library ([`vfx_preset`]) holds
//! the effects every game needs — sparks, muzzle flash, impacts per surface,
//! explosions, tyre smoke, splashes, fire, weather — and a script may name
//! one, override it, or hand over a whole preset of its own.

use crate::particles::{EmitterAnchor, ParticleKind, ParticleSpec};
use makepad_math::*;
use std::sync::{Arc, OnceLock};

/// What a particle's quad looks like. Some are procedural in the pixel
/// shader, the rest are cells of the renderer's generated flipbook atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VfxSprite {
    /// Soft round glow.
    Dot,
    /// Hot streak: stretch it along its velocity.
    Spark,
    /// Billowing smoke puff (lit flipbook).
    Smoke,
    /// Thin curling smoke (lit flipbook): gun smoke, steam.
    Wisp,
    /// Flame tongues (emissive flipbook).
    Fire,
    /// Smoke that starts as burning gas and cools to soot over its life.
    Fireball,
    /// Irregular lit chip: rock, wood, metal debris.
    Debris,
    /// Thin bright ring: shockwaves, ripples.
    Ring,
    /// Liquid drop with a highlight: water, blood.
    Droplet,
    /// Star-shaped muzzle / impact flash.
    Flash,
    /// Four-point twinkle: magic.
    Star,
    /// A falling rain streak.
    Streak,
    /// A snowflake.
    Flake,
    /// Lumpy lit dust clump: impacts on dirt, footfalls.
    Dust,
}

impl VfxSprite {
    pub const NAMES: &'static [&'static str] = &[
        "dot", "spark", "smoke", "wisp", "fire", "fireball", "debris", "ring", "droplet", "flash", "star",
        "streak", "flake", "dust",
    ];

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "dot" | "glow" => Self::Dot,
            "spark" => Self::Spark,
            "smoke" => Self::Smoke,
            "wisp" => Self::Wisp,
            "fire" | "flame" => Self::Fire,
            "fireball" => Self::Fireball,
            "debris" | "chip" => Self::Debris,
            "ring" | "shockwave" => Self::Ring,
            "droplet" | "drop" => Self::Droplet,
            "flash" => Self::Flash,
            "star" => Self::Star,
            "streak" | "rain" => Self::Streak,
            "flake" | "snow" => Self::Flake,
            "dust" => Self::Dust,
            _ => return None,
        })
    }

    /// The shader's sprite lane.
    pub fn id(self) -> f32 {
        self as u32 as f32
    }
}

/// How a particle composites. Both are premultiplied: `Add` writes zero
/// alpha, so it only ever adds light.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VfxBlend {
    Add,
    Alpha,
}

/// One particle layer of an effect. Sizes are full diameters in metres,
/// speeds m/s, times seconds; `scale` on the request multiplies lengths.
#[derive(Clone, Debug, PartialEq)]
pub struct VfxLayer {
    pub sprite: VfxSprite,
    pub blend: VfxBlend,
    /// Particles per one-shot burst.
    pub count: f32,
    /// Particles per second while a looping effect runs (0 = burst only).
    pub rate: f32,
    /// Seconds a burst's births are spread over (0 = all at once).
    pub duration: f32,
    /// Seconds before this layer starts.
    pub delay: f32,
    /// Lifetime range; each particle picks one.
    pub life: (f32, f32),
    pub speed: (f32, f32),
    /// Diameter at birth and at death.
    pub size: (f32, f32),
    /// Colour (sRGB) and alpha at birth and at death.
    pub color: Vec4f,
    pub color_end: Vec4f,
    /// Emissive strength: 1 = display white, above 1 blooms. 0 = no glow.
    pub intensity: f32,
    /// 0 = self-lit, 1 = lit by the sun and sky (smoke, dust, debris).
    pub lit: f32,
    /// Launch cone half-angle around the effect's `dir`, radians (π = all
    /// directions).
    pub spread: f32,
    /// Downward acceleration m/s² (negative rises: hot smoke).
    pub gravity: f32,
    /// Linear air drag per second.
    pub drag: f32,
    /// Seconds of velocity the quad stretches along (sparks, rain).
    pub stretch: f32,
    /// Max spin, radians per second.
    pub spin: f32,
    /// Spawn sphere radius.
    pub radius: f32,
    /// Lateral wander amplitude, metres (snow drift, curling smoke).
    pub wobble: f32,
    /// Soft-particle depth fade distance, metres.
    pub soft: f32,
    /// Share of the life spent fading in.
    pub fade_in: f32,
    /// Stop at the ground under the effect instead of falling through.
    pub collide: bool,
    /// Lie in the plane across `dir` instead of facing the camera (ground
    /// rings, water ripples).
    pub flat: bool,
    /// Take the request's `color` (glowing layers do by default; smoke and
    /// debris keep their own).
    pub tint: bool,
    /// Share of a moving emitter's velocity particles keep.
    pub inherit: f32,
    /// A looping layer starts already full: one burst of `rate × life`
    /// particles at spread ages, so a cloud bank or a fire is there from the
    /// first frame instead of growing in over one lifetime.
    pub prewarm: bool,
}

impl Default for VfxLayer {
    fn default() -> Self {
        Self {
            sprite: VfxSprite::Dot,
            blend: VfxBlend::Add,
            count: 8.0,
            rate: 0.0,
            duration: 0.0,
            delay: 0.0,
            life: (0.4, 0.6),
            speed: (1.0, 2.0),
            size: (0.2, 0.1),
            color: vec4f(1.0, 0.9, 0.7, 1.0),
            color_end: vec4f(1.0, 0.5, 0.2, 0.0),
            intensity: 1.0,
            lit: 0.0,
            spread: 0.6,
            gravity: 0.0,
            drag: 0.5,
            stretch: 0.0,
            spin: 0.0,
            radius: 0.0,
            wobble: 0.0,
            soft: 0.25,
            fade_in: 0.0,
            collide: false,
            flat: false,
            tint: true,
            inherit: 0.0,
            prewarm: false,
        }
    }
}

impl VfxLayer {
    /// The layer an old `game.particles`/`game.burst` spec draws as: sparks
    /// become hot stretched streaks, smoke and dust lit soft puffs, trails
    /// glows. The spec's own numbers keep their meaning.
    pub fn from_spec(spec: &ParticleSpec) -> Self {
        let (_, _, _, _, drag) = spec.kind.defaults();
        // The old launch vector was (sx·spread, 1 − 0.3·spread + 0.3·sy,
        // sz·spread): a cone about up whose half-angle grows with spread.
        let spread = (spec.spread.max(0.0).atan() * 1.35).min(std::f32::consts::PI);
        let s = spec.size;
        let life = (spec.life * 0.75, spec.life * 1.25);
        let speed = spec.speed.abs();
        // The old sim used 22 m/s² scaled by the spec's gravity.
        let gravity = spec.gravity * 22.0;
        let c = spec.color;
        let base = VfxLayer {
            count: spec.rate,
            rate: spec.rate,
            life,
            speed: (speed * 0.7, speed * 1.3),
            spread,
            gravity,
            drag,
            color: c,
            ..VfxLayer::default()
        };
        match spec.kind {
            ParticleKind::Spark => VfxLayer {
                sprite: VfxSprite::Spark,
                blend: VfxBlend::Add,
                size: (s * 0.9, s * 0.35),
                color_end: vec4f(c.x, c.y * 0.6, c.z * 0.3, 0.0),
                intensity: 4.0,
                stretch: 0.035,
                collide: gravity > 0.0,
                soft: 0.05,
                ..base
            },
            ParticleKind::Smoke => VfxLayer {
                sprite: VfxSprite::Smoke,
                blend: VfxBlend::Alpha,
                size: (s * 1.6, s * 4.0),
                color_end: vec4f(c.x, c.y, c.z, 0.0),
                intensity: 0.0,
                lit: 0.85,
                spin: 0.8,
                fade_in: 0.12,
                soft: 0.4,
                tint: false,
                ..base
            },
            ParticleKind::Dust => VfxLayer {
                sprite: VfxSprite::Dust,
                blend: VfxBlend::Alpha,
                size: (s * 1.6, s * 3.6),
                color_end: vec4f(c.x, c.y, c.z, 0.0),
                intensity: 0.0,
                lit: 0.9,
                spin: 0.6,
                fade_in: 0.08,
                soft: 0.3,
                collide: true,
                tint: false,
                ..base
            },
            ParticleKind::Trail => VfxLayer {
                sprite: VfxSprite::Dot,
                blend: VfxBlend::Add,
                size: (s * 1.4, s * 0.6),
                color_end: vec4f(c.x, c.y, c.z, 0.0),
                intensity: 2.0,
                soft: 0.1,
                ..base
            },
        }
    }
}

/// A light the effect throws for its first moments (through the renderer's
/// frame lights, so the world around a muzzle or a blast lights up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VfxLight {
    /// sRGB tint.
    pub color: Vec3f,
    /// Peak strength in lamp units.
    pub intensity: f32,
    pub radius: f32,
    /// Seconds to fade out (looping effects hold it and flicker).
    pub life: f32,
}

/// A device-local surface mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VfxDecalKind {
    Bullet,
    Scorch,
    Blood,
    Dirt,
    Skid,
    Crack,
    Energy,
    Wet,
}

impl VfxDecalKind {
    pub const NAMES: &'static [&'static str] =
        &["bullet", "scorch", "blood", "dirt", "skid", "crack", "energy", "wet"];

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "bullet" | "bullet_hole" | "hole" => Self::Bullet,
            "scorch" | "burn" => Self::Scorch,
            "blood" => Self::Blood,
            "dirt" | "dust" | "mud" => Self::Dirt,
            "skid" | "skid_mark" | "tyre" | "tire" => Self::Skid,
            "crack" => Self::Crack,
            "energy" | "plasma" => Self::Energy,
            "wet" | "water" => Self::Wet,
            _ => return None,
        })
    }

    pub fn id(self) -> f32 {
        self as u32 as f32
    }

    /// (full size in metres, seconds before it fades away).
    pub fn defaults(self) -> (f32, f32) {
        match self {
            Self::Bullet => (0.08, 30.0),
            Self::Scorch => (2.5, 40.0),
            Self::Blood => (0.6, 30.0),
            Self::Dirt => (0.5, 20.0),
            Self::Skid => (0.24, 20.0),
            Self::Crack => (0.5, 40.0),
            Self::Energy => (0.18, 20.0),
            Self::Wet => (0.8, 12.0),
        }
    }
}

/// The mark an effect leaves where it hit (needs a surface normal).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VfxDecal {
    pub kind: VfxDecalKind,
    pub size: f32,
    pub life: f32,
    pub color: Vec4f,
}

impl VfxDecal {
    pub fn new(kind: VfxDecalKind) -> Self {
        let (size, life) = kind.defaults();
        Self { kind, size, life, color: vec4f(1.0, 1.0, 1.0, 1.0) }
    }
}

/// A whole effect as data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VfxPreset {
    pub layers: Vec<VfxLayer>,
    pub light: Option<VfxLight>,
    pub decal: Option<VfxDecal>,
    /// Runs until stopped (fire, tyre smoke, rain); its layers emit `rate`
    /// per second instead of one `count` burst.
    pub looping: bool,
}

/// A preset by name (resolved device-side, script-defined names first) or
/// carried inline.
#[derive(Clone, Debug, PartialEq)]
pub enum VfxPresetRef {
    Named(String),
    Inline(Arc<VfxPreset>),
}

/// One `game.vfx` call.
#[derive(Clone, Debug, PartialEq)]
pub struct VfxRequest {
    pub preset: VfxPresetRef,
    pub anchor: EmitterAnchor,
    /// Launch axis: a barrel's forward, a hit's surface normal. Unit.
    pub dir: Vec3f,
    /// Surface normal for the preset's decal; no normal, no mark.
    pub normal: Option<Vec3f>,
    pub scale: f32,
    /// Replaces the colour of tintable layers and the light.
    pub color: Option<Vec4f>,
    /// Multiplies emissive intensity and the light.
    pub intensity: f32,
    /// Multiplies particle counts and rates.
    pub density: f32,
    /// Emitter handle for looping presets (0 = one-shot).
    pub id: u64,
}

impl VfxRequest {
    pub fn named(name: &str, at: Vec3f) -> Self {
        Self {
            preset: VfxPresetRef::Named(name.to_string()),
            anchor: EmitterAnchor::Point(at),
            dir: vec3f(0.0, 1.0, 0.0),
            normal: None,
            scale: 1.0,
            color: None,
            intensity: 1.0,
            density: 1.0,
            id: 0,
        }
    }
}

/// One surface mark. Marks with the same non-zero `chain` join into a strip
/// (skid marks): each new point draws the segment from the chain's last one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecalRequest {
    pub kind: VfxDecalKind,
    pub pos: Vec3f,
    pub normal: Vec3f,
    /// In-plane direction (a skid's travel); zero = random roll.
    pub dir: Vec3f,
    pub size: f32,
    pub life: f32,
    pub color: Vec4f,
    pub chain: u64,
}

// ---------------------------------------------------------------------------
// The built-in library.

const PI: f32 = std::f32::consts::PI;

fn rgb(hex: u32, a: f32) -> Vec4f {
    vec4f(
        ((hex >> 16) & 255) as f32 / 255.0,
        ((hex >> 8) & 255) as f32 / 255.0,
        (hex & 255) as f32 / 255.0,
        a,
    )
}

fn light(hex: u32, intensity: f32, radius: f32, life: f32) -> Option<VfxLight> {
    let c = rgb(hex, 1.0);
    Some(VfxLight { color: vec3f(c.x, c.y, c.z), intensity, radius, life })
}

fn sparks(count: f32, speed: (f32, f32), hex: u32, hex_end: u32) -> VfxLayer {
    VfxLayer {
        sprite: VfxSprite::Spark,
        count,
        life: (0.18, 0.45),
        speed,
        size: (0.035, 0.012),
        color: rgb(hex, 1.0),
        color_end: rgb(hex_end, 0.0),
        intensity: 7.0,
        spread: 0.9,
        gravity: 9.0,
        drag: 1.2,
        stretch: 0.04,
        soft: 0.03,
        collide: true,
        ..VfxLayer::default()
    }
}

fn flash(size: f32, life: f32, hex: u32, intensity: f32) -> VfxLayer {
    VfxLayer {
        sprite: VfxSprite::Flash,
        count: 1.0,
        life: (life, life),
        speed: (0.0, 0.0),
        size: (size, size * 1.3),
        color: rgb(hex, 1.0),
        color_end: rgb(hex, 0.0),
        intensity,
        spread: 0.0,
        drag: 0.0,
        spin: 0.0,
        soft: 0.08,
        ..VfxLayer::default()
    }
}

fn glow(size: f32, life: f32, hex: u32, intensity: f32) -> VfxLayer {
    VfxLayer { sprite: VfxSprite::Dot, ..flash(size, life, hex, intensity) }
}

fn smoke(count: f32, hex: u32, alpha: f32, size: (f32, f32), life: (f32, f32), speed: (f32, f32)) -> VfxLayer {
    VfxLayer {
        sprite: VfxSprite::Smoke,
        blend: VfxBlend::Alpha,
        count,
        life,
        speed,
        size,
        color: rgb(hex, alpha),
        color_end: rgb(hex, 0.0),
        intensity: 0.0,
        lit: 0.9,
        spread: 0.8,
        gravity: -0.6,
        drag: 2.2,
        spin: 0.6,
        radius: 0.05,
        soft: 0.5,
        fade_in: 0.1,
        tint: false,
        ..VfxLayer::default()
    }
}

fn dust(count: f32, hex: u32, alpha: f32, size: (f32, f32), speed: (f32, f32)) -> VfxLayer {
    VfxLayer {
        sprite: VfxSprite::Dust,
        blend: VfxBlend::Alpha,
        count,
        life: (0.6, 1.3),
        speed,
        size,
        color: rgb(hex, alpha),
        color_end: rgb(hex, 0.0),
        intensity: 0.0,
        lit: 0.95,
        spread: 0.75,
        gravity: 1.2,
        drag: 3.0,
        spin: 0.5,
        soft: 0.3,
        fade_in: 0.06,
        collide: true,
        tint: false,
        ..VfxLayer::default()
    }
}

fn debris(count: f32, hex: u32, size: f32, speed: (f32, f32)) -> VfxLayer {
    VfxLayer {
        sprite: VfxSprite::Debris,
        blend: VfxBlend::Alpha,
        count,
        life: (0.7, 1.4),
        speed,
        size: (size, size),
        color: rgb(hex, 1.0),
        color_end: rgb(hex, 0.0),
        intensity: 0.0,
        lit: 1.0,
        spread: 0.8,
        gravity: 9.8,
        drag: 0.4,
        spin: 12.0,
        soft: 0.02,
        collide: true,
        tint: false,
        ..VfxLayer::default()
    }
}

fn droplets(count: f32, hex: u32, alpha: f32, size: f32, speed: (f32, f32)) -> VfxLayer {
    VfxLayer {
        sprite: VfxSprite::Droplet,
        blend: VfxBlend::Alpha,
        count,
        life: (0.4, 0.9),
        speed,
        size: (size, size * 0.7),
        color: rgb(hex, alpha),
        color_end: rgb(hex, 0.0),
        intensity: 0.0,
        lit: 0.7,
        spread: 0.7,
        gravity: 9.8,
        drag: 0.6,
        stretch: 0.025,
        soft: 0.05,
        collide: true,
        tint: false,
        ..VfxLayer::default()
    }
}

fn ring(size: (f32, f32), life: f32, hex: u32, alpha: f32, intensity: f32, flat: bool) -> VfxLayer {
    VfxLayer {
        sprite: VfxSprite::Ring,
        count: 1.0,
        life: (life, life),
        speed: (0.0, 0.0),
        size,
        color: rgb(hex, alpha),
        color_end: rgb(hex, 0.0),
        intensity,
        spread: 0.0,
        drag: 0.0,
        soft: 0.1,
        flat,
        ..VfxLayer::default()
    }
}

fn impact(surface: &str) -> VfxPreset {
    let bullet = Some(VfxDecal::new(VfxDecalKind::Bullet));
    match surface {
        "metal" => VfxPreset {
            layers: vec![
                sparks(26.0, (3.0, 9.0), 0xfff0c8, 0xff6a18),
                flash(0.22, 0.05, 0xfff4d8, 6.0),
                smoke(2.0, 0x8a8680, 0.35, (0.08, 0.5), (0.4, 0.8), (0.4, 1.0)),
            ],
            light: light(0xffc070, 1.5, 3.0, 0.06),
            decal: bullet,
            looping: false,
        },
        "wood" => VfxPreset {
            layers: vec![
                debris(10.0, 0x8a5a2e, 0.035, (2.0, 5.0)),
                dust(5.0, 0xa88862, 0.55, (0.1, 0.55), (0.6, 2.0)),
            ],
            light: None,
            decal: bullet,
            looping: false,
        },
        "dirt" | "sand" | "grass" => VfxPreset {
            layers: vec![
                dust(10.0, 0x9a7d58, 0.75, (0.15, 0.9), (1.0, 3.5)),
                debris(8.0, 0x6a5238, 0.03, (2.0, 5.0)),
            ],
            light: None,
            decal: Some(VfxDecal::new(VfxDecalKind::Dirt)),
            looping: false,
        },
        "water" => water_splash(0.6),
        "flesh" | "blood" => VfxPreset {
            layers: vec![
                droplets(16.0, 0x7a0a0a, 0.95, 0.045, (1.5, 4.5)),
                VfxLayer { color: rgb(0x8a1010, 0.45), ..smoke(4.0, 0x8a1010, 0.45, (0.08, 0.45), (0.25, 0.45), (0.5, 1.5)) },
            ],
            light: None,
            decal: Some(VfxDecal::new(VfxDecalKind::Blood)),
            looping: false,
        },
        "glass" => VfxPreset {
            layers: vec![
                VfxLayer { lit: 0.6, intensity: 1.5, ..debris(14.0, 0xd8f0ff, 0.025, (1.5, 4.0)) },
                flash(0.15, 0.04, 0xe8f6ff, 4.0),
            ],
            light: None,
            decal: Some(VfxDecal::new(VfxDecalKind::Crack)),
            looping: false,
        },
        // Concrete, stone, plaster: the default.
        _ => VfxPreset {
            layers: vec![
                dust(8.0, 0xb0a590, 0.7, (0.1, 0.7), (0.8, 2.8)),
                debris(7.0, 0x8c8578, 0.028, (2.0, 5.5)),
                sparks(4.0, (2.0, 5.0), 0xffe0b0, 0xff7020),
            ],
            light: None,
            decal: bullet,
            looping: false,
        },
    }
}

fn water_splash(scale: f32) -> VfxPreset {
    VfxPreset {
        layers: vec![
            VfxLayer { spread: 0.45, ..droplets(34.0 * scale, 0xd8ecff, 0.8, 0.07, (2.5, 6.5)) },
            VfxLayer {
                sprite: VfxSprite::Wisp,
                lit: 0.8,
                ..smoke(8.0 * scale, 0xeaf4ff, 0.5, (0.2, 1.2), (0.5, 1.0), (1.0, 2.5))
            },
            ring((0.3, 2.4), 0.8, 0xeaf4ff, 0.7, 0.8, true),
        ],
        light: None,
        decal: Some(VfxDecal::new(VfxDecalKind::Wet)),
        looping: false,
    }
}

fn explosion() -> VfxPreset {
    VfxPreset {
        layers: vec![
            flash(5.0, 0.12, 0xfff2d0, 12.0),
            VfxLayer {
                sprite: VfxSprite::Fireball,
                blend: VfxBlend::Alpha,
                count: 20.0,
                duration: 0.08,
                life: (0.7, 1.3),
                speed: (2.0, 7.0),
                size: (1.2, 3.6),
                color: rgb(0xffe6a8, 1.0),
                color_end: rgb(0x2a2420, 0.0),
                intensity: 9.0,
                lit: 0.8,
                spread: PI,
                gravity: -1.5,
                drag: 3.5,
                spin: 1.0,
                radius: 0.4,
                soft: 0.8,
                ..VfxLayer::default()
            },
            VfxLayer {
                delay: 0.12,
                duration: 0.4,
                spread: 1.2,
                gravity: -1.4,
                ..smoke(26.0, 0x3a3632, 0.85, (1.6, 5.5), (2.5, 4.5), (1.5, 4.5))
            },
            VfxLayer { spread: 1.1, life: (1.2, 2.4), ..debris(26.0, 0x4a4038, 0.12, (7.0, 17.0)) },
            VfxLayer { count: 70.0, spread: 1.4, life: (0.5, 1.3), ..sparks(70.0, (9.0, 24.0), 0xffe6b0, 0xff5010) },
            ring((0.6, 16.0), 0.45, 0xfff4e0, 0.55, 2.0, true),
            VfxLayer { spread: 1.3, ..dust(18.0, 0x9a8a74, 0.6, (1.0, 4.5), (3.0, 8.0)) },
        ],
        light: light(0xffa050, 14.0, 24.0, 0.9),
        decal: Some(VfxDecal::new(VfxDecalKind::Scorch)),
        looping: false,
    }
}

fn build_library() -> Vec<(&'static str, Arc<VfxPreset>)> {
    let one = |layers: Vec<VfxLayer>, light: Option<VfxLight>| VfxPreset { layers, light, decal: None, looping: false };
    let looping = |layers: Vec<VfxLayer>, light: Option<VfxLight>| VfxPreset { layers, light, decal: None, looping: true };
    let mut v: Vec<(&'static str, VfxPreset)> = vec![
        ("hit_spark", one(vec![
            sparks(22.0, (3.0, 8.0), 0xfff0c8, 0xff7020),
            flash(0.45, 0.07, 0xfff6e0, 7.0),
            glow(0.7, 0.12, 0xffd890, 2.5),
        ], light(0xffc880, 2.0, 3.5, 0.08))),
        ("hit_heavy", one(vec![
            sparks(46.0, (4.0, 12.0), 0xfff0c8, 0xff6010),
            flash(0.9, 0.09, 0xfff6e0, 9.0),
            glow(1.3, 0.16, 0xffc070, 3.0),
            ring((0.3, 2.2), 0.22, 0xfff4e0, 0.8, 3.0, false),
            smoke(5.0, 0xd8cfc0, 0.35, (0.2, 1.1), (0.35, 0.6), (0.8, 2.0)),
        ], light(0xffb870, 4.0, 5.0, 0.12))),
        ("block_spark", one(vec![
            sparks(16.0, (2.5, 6.0), 0xc8f0ff, 0x3080ff),
            flash(0.5, 0.07, 0xd8f4ff, 6.0),
            ring((0.2, 1.1), 0.16, 0x9adcff, 0.9, 3.0, false),
        ], light(0x80c8ff, 1.5, 3.0, 0.08))),
        ("muzzle_flash", one(vec![
            VfxLayer { stretch: 0.012, speed: (5.0, 9.0), spread: 0.18, life: (0.03, 0.05), size: (0.1, 0.06),
                intensity: 9.0, sprite: VfxSprite::Flash, count: 3.0, drag: 8.0, ..flash(0.1, 0.04, 0xffd898, 9.0) },
            flash(0.22, 0.045, 0xfff0c8, 10.0),
            glow(0.5, 0.06, 0xffb060, 3.0),
            VfxLayer { count: 5.0, spread: 0.35, gravity: 3.0, collide: false, ..sparks(5.0, (6.0, 14.0), 0xffe0a0, 0xff6010) },
            VfxLayer { sprite: VfxSprite::Wisp, spread: 0.5, gravity: -0.5, drag: 4.0,
                ..smoke(3.0, 0xb8b4ae, 0.3, (0.05, 0.4), (0.5, 1.0), (0.6, 1.6)) },
        ], light(0xffb060, 3.5, 6.0, 0.06))),
        ("shell_smoke", one(vec![
            VfxLayer { sprite: VfxSprite::Wisp, spread: 0.4, gravity: -0.8, drag: 2.5, wobble: 0.05,
                ..smoke(4.0, 0xc8c4be, 0.35, (0.05, 0.5), (0.8, 1.6), (0.3, 0.9)) },
        ], None)),
        ("impact", impact("concrete")),
        ("impact_concrete", impact("concrete")),
        ("impact_metal", impact("metal")),
        ("impact_wood", impact("wood")),
        ("impact_dirt", impact("dirt")),
        ("impact_water", impact("water")),
        ("impact_flesh", impact("flesh")),
        ("impact_glass", impact("glass")),
        ("blood", impact("flesh")),
        ("impact_dust", one(vec![dust(12.0, 0x9a8a74, 0.7, (0.2, 1.1), (0.8, 2.6))], None)),
        ("explosion", explosion()),
        ("explosion_small", {
            let mut p = explosion();
            for l in p.layers.iter_mut() {
                l.count = (l.count * 0.45).max(1.0);
                l.size = (l.size.0 * 0.5, l.size.1 * 0.5);
                l.speed = (l.speed.0 * 0.55, l.speed.1 * 0.55);
            }
            p.light = light(0xffa050, 7.0, 12.0, 0.6);
            p.decal = Some(VfxDecal { size: 1.2, ..VfxDecal::new(VfxDecalKind::Scorch) });
            p
        }),
        ("shockwave", one(vec![ring((0.5, 12.0), 0.5, 0xfff4e0, 0.7, 2.5, true)], None)),
        ("smoke", one(vec![smoke(10.0, 0x6e6a66, 0.6, (0.4, 2.2), (1.5, 3.0), (0.4, 1.4))], None)),
        ("smoke_puff", one(vec![smoke(6.0, 0xd8d4ce, 0.5, (0.2, 1.1), (0.5, 1.0), (0.5, 1.5))], None)),
        ("dust", one(vec![dust(14.0, 0xa89478, 0.6, (0.2, 1.2), (0.6, 2.0))], None)),
        ("land_dust", one(vec![VfxLayer { spread: 1.5, gravity: 0.6, ..dust(16.0, 0xa89478, 0.55, (0.2, 1.0), (1.0, 2.6)) }], None)),
        ("tyre_smoke", VfxPreset {
            layers: vec![VfxLayer {
                count: 2.0,
                rate: 34.0,
                spread: 1.0,
                gravity: -0.35,
                drag: 1.4,
                inherit: 0.25,
                wobble: 0.15,
                life: (1.6, 3.2),
                ..smoke(2.0, 0xe4e2de, 0.5, (0.35, 3.2), (1.6, 3.2), (0.4, 1.6))
            }],
            light: None,
            decal: None,
            looping: true,
        }),
        ("skid", VfxPreset { layers: Vec::new(), light: None, decal: Some(VfxDecal::new(VfxDecalKind::Skid)), looping: false }),
        ("dust_trail", VfxPreset {
            layers: vec![VfxLayer { count: 2.0, rate: 30.0, inherit: 0.15, life: (0.9, 1.8), ..dust(2.0, 0xb09a78, 0.5, (0.3, 1.8), (0.3, 1.2)) }],
            light: None,
            decal: None,
            looping: true,
        }),
        ("water_splash", water_splash(1.0)),
        ("magic", one(vec![
            VfxLayer { sprite: VfxSprite::Star, count: 26.0, life: (0.6, 1.2), speed: (0.5, 2.2), size: (0.16, 0.02),
                color: rgb(0xd8a0ff, 1.0), color_end: rgb(0x6030ff, 0.0), intensity: 6.0, spread: PI, gravity: -1.0,
                drag: 1.5, spin: 4.0, wobble: 0.12, radius: 0.2, ..VfxLayer::default() },
            glow(1.1, 0.35, 0xb070ff, 3.0),
            ring((0.2, 2.0), 0.4, 0xc890ff, 0.8, 3.0, false),
        ], light(0xa060ff, 3.0, 6.0, 0.35))),
        ("energy", one(vec![
            VfxLayer { stretch: 0.03, ..sparks(24.0, (4.0, 10.0), 0xe0ffff, 0x20a0ff) },
            flash(0.6, 0.08, 0xd0f8ff, 9.0),
            ring((0.2, 1.6), 0.2, 0x80e0ff, 0.9, 4.0, false),
            glow(0.9, 0.2, 0x60c8ff, 3.0),
        ], light(0x60c0ff, 3.0, 5.0, 0.15))),
        ("fire", looping(vec![
            VfxLayer { sprite: VfxSprite::Fire, count: 4.0, rate: 34.0, life: (0.45, 0.85), speed: (0.4, 1.2),
                size: (0.7, 0.25), color: rgb(0xff9020, 1.0), color_end: rgb(0xd02004, 0.0), intensity: 1.1,
                spread: 0.3, gravity: -2.8, drag: 1.5, spin: 0.4, radius: 0.18, soft: 0.3, fade_in: 0.1,
                wobble: 0.04, ..VfxLayer::default() },
            VfxLayer { count: 1.0, rate: 7.0, life: (0.8, 1.8), gravity: -1.6, drag: 0.8, spread: 0.5, size: (0.03, 0.01),
                wobble: 0.2, collide: false, stretch: 0.02, ..sparks(1.0, (0.8, 2.2), 0xffc070, 0xff4010) },
            VfxLayer { count: 1.0, rate: 7.0, delay: 0.2, gravity: -1.3, wobble: 0.2,
                ..smoke(1.0, 0x3a3634, 0.45, (0.5, 2.6), (2.0, 3.5), (0.4, 1.0)) },
        ], light(0xff9040, 3.0, 8.0, 0.0))),
        ("sparks_shower", one(vec![VfxLayer { spread: 0.5, life: (0.6, 1.2), ..sparks(60.0, (2.0, 7.0), 0xfff0c0, 0xff6010) }], None)),
        ("confetti", one(vec![
            VfxLayer { sprite: VfxSprite::Debris, blend: VfxBlend::Alpha, count: 60.0, life: (1.5, 3.0), speed: (4.0, 9.0),
                size: (0.07, 0.07), color: rgb(0xffd040, 1.0), color_end: rgb(0xff4080, 0.0), intensity: 0.4, lit: 0.6,
                spread: 0.7, gravity: 3.0, drag: 1.8, spin: 10.0, wobble: 0.3, collide: true, ..VfxLayer::default() },
        ], None)),
        // Cumulus: big lit puffs that cycle slowly (each lives ~1.5 min and
        // evolves through the smoke flipbook), prewarmed so the sky starts
        // full. Sun-lit tops, sky-lit (blue-grey) undersides, soft edges
        // against terrain and each other. ~200 m across at scale 1.
        ("cloud", VfxPreset {
            layers: vec![VfxLayer {
                sprite: VfxSprite::Smoke,
                blend: VfxBlend::Alpha,
                count: 0.0,
                rate: 0.3,
                life: (70.0, 110.0),
                speed: (0.0, 0.5),
                size: (70.0, 115.0),
                color: rgb(0xf6f8fb, 0.85),
                color_end: rgb(0xe6eaf0, 0.0),
                intensity: 0.0,
                lit: 1.0,
                spread: PI,
                gravity: 0.0,
                drag: 0.3,
                spin: 0.01,
                radius: 55.0,
                soft: 30.0,
                fade_in: 0.25,
                tint: false,
                prewarm: true,
                ..VfxLayer::default()
            }],
            light: None,
            decal: None,
            looping: true,
        }),
        ("rain", VfxPreset {
            layers: vec![VfxLayer {
                sprite: VfxSprite::Streak,
                blend: VfxBlend::Alpha,
                count: 20.0,
                rate: 900.0,
                life: (0.9, 1.2),
                speed: (11.0, 13.0),
                size: (0.018, 0.018),
                color: rgb(0xb8c8dc, 0.4),
                color_end: rgb(0xb8c8dc, 0.3),
                intensity: 0.0,
                lit: 0.7,
                spread: 0.05,
                gravity: 0.0,
                drag: 0.0,
                stretch: 0.035,
                radius: 18.0,
                soft: 0.05,
                collide: true,
                tint: false,
                ..VfxLayer::default()
            }],
            light: None,
            decal: None,
            looping: true,
        }),
        ("snow", VfxPreset {
            layers: vec![VfxLayer {
                sprite: VfxSprite::Flake,
                blend: VfxBlend::Alpha,
                count: 20.0,
                rate: 260.0,
                life: (5.0, 7.0),
                speed: (0.7, 1.3),
                size: (0.035, 0.035),
                color: rgb(0xffffff, 0.9),
                color_end: rgb(0xffffff, 0.7),
                intensity: 0.0,
                lit: 0.8,
                spread: 0.2,
                gravity: 0.0,
                drag: 0.0,
                spin: 2.0,
                wobble: 0.35,
                radius: 16.0,
                soft: 0.05,
                fade_in: 0.05,
                collide: true,
                tint: false,
                ..VfxLayer::default()
            }],
            light: None,
            decal: None,
            looping: true,
        }),
    ];
    v.sort_by(|a, b| a.0.cmp(b.0));
    v.into_iter().map(|(n, p)| (n, Arc::new(p))).collect()
}

fn library() -> &'static [(&'static str, Arc<VfxPreset>)] {
    static LIB: OnceLock<Vec<(&'static str, Arc<VfxPreset>)>> = OnceLock::new();
    LIB.get_or_init(build_library)
}

/// A built-in preset by name. `impact_<surface>` falls back to concrete.
pub fn vfx_preset(name: &str) -> Option<Arc<VfxPreset>> {
    let lib = library();
    if let Ok(i) = lib.binary_search_by(|(n, _)| (*n).cmp(name)) {
        return Some(lib[i].1.clone());
    }
    name.strip_prefix("impact_").map(|_| lib[lib.binary_search_by(|(n, _)| (*n).cmp("impact")).unwrap()].1.clone())
}

/// Every built-in preset name, sorted.
pub fn vfx_preset_names() -> impl Iterator<Item = &'static str> {
    library().iter().map(|(n, _)| *n)
}

/// Presets whose natural axis is DOWN (weather): a request without an
/// explicit `dir` uses this instead of up.
pub fn vfx_default_dir(name: &str) -> Vec3f {
    match name {
        "rain" | "snow" => vec3f(0.0, -1.0, 0.0),
        _ => vec3f(0.0, 1.0, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_has_every_effect_games_ask_for() {
        for name in [
            "hit_spark", "muzzle_flash", "shell_smoke", "impact_metal", "impact_dirt", "impact_flesh", "explosion",
            "tyre_smoke", "skid", "dust_trail", "water_splash", "magic", "energy", "fire", "rain", "snow", "cloud",
        ] {
            assert!(vfx_preset(name).is_some(), "missing preset {name}");
        }
        assert!(vfx_preset("impact_lava").is_some(), "unknown surfaces fall back to the default impact");
        assert!(vfx_preset("nope").is_none());
        let names: Vec<_> = vfx_preset_names().collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn muzzle_flash_lights_the_scene_and_explosions_scorch() {
        assert!(vfx_preset("muzzle_flash").unwrap().light.is_some());
        let e = vfx_preset("explosion").unwrap();
        assert_eq!(e.decal.map(|d| d.kind), Some(VfxDecalKind::Scorch));
        assert!(e.layers.iter().any(|l| l.sprite == VfxSprite::Fireball));
        assert!(e.layers.iter().any(|l| l.flat && l.sprite == VfxSprite::Ring), "a ground shockwave");
        assert!(vfx_preset("fire").unwrap().looping);
    }

    #[test]
    fn legacy_specs_keep_their_numbers() {
        let mut s = ParticleSpec::new(ParticleKind::Spark);
        s.rate = 12.0;
        s.life = 0.5;
        let l = VfxLayer::from_spec(&s);
        assert_eq!(l.count, 12.0);
        assert_eq!(l.sprite, VfxSprite::Spark);
        assert!(l.stretch > 0.0 && l.blend == VfxBlend::Add);
        assert!((l.life.0 - 0.375).abs() < 1e-6 && (l.life.1 - 0.625).abs() < 1e-6);
        let smoke = VfxLayer::from_spec(&ParticleSpec::new(ParticleKind::Smoke));
        assert!(smoke.gravity < 0.0, "smoke still rises");
        assert_eq!(smoke.blend, VfxBlend::Alpha);
    }
}
