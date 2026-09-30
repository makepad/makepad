//! The record layouts physics exchanges with kernels (KERNELS.md §3.5.4
//! item 5): `Body` goes into physics (a kernel's `output(Body)` or
//! `emit_buffer(Body, n)` is a spawn), `Rigid`, `Contact` and `Event` come
//! out of it (`input(Rigid)` reads `bodies("g")`, `input(Contact)` reads
//! `contacts("g")`).
//!
//! Records are zero-filled before a kernel writes them, so every field's
//! zero is its default: a zero `rot` is the identity, a zero `density`,
//! `friction`, `color` or `scale` is the default value, and so on.

use makepad_render_kernels::compute::kernel::{FieldTy, Layout, LayoutField};

fn field(name: &str, ty: FieldTy, offset: u32) -> LayoutField {
    LayoutField { name: name.into(), ty, offset }
}

/// Words per `Body` record.
pub const BODY_WORDS: usize = 32;
/// Words per `Rigid` record.
pub const RIGID_WORDS: usize = 28;
/// Words per `Contact` record.
pub const CONTACT_WORDS: usize = 12;
/// Words per `Event` record.
pub const EVENT_WORDS: usize = 8;

/// A body to create: where, how big, how it moves, how it looks.
pub fn body_layout() -> Layout {
    use FieldTy::*;
    Layout {
        name: "Body".into(),
        stride: BODY_WORDS as u32,
        fields: vec![
            field("pos", Vec3, 0),
            field("size", Vec3, 3),
            field("rot", Vec4, 6),
            field("vel", Vec3, 10),
            field("spin", Vec3, 13),
            field("drag", F32, 16),
            field("spin_drag", F32, 17),
            field("density", F32, 18),
            field("friction", F32, 19),
            field("bounce", F32, 20),
            field("lift", F32, 21),
            field("delay", F32, 22),
            field("life", F32, 23),
            field("color", Vec4, 24),
            field("scale", Vec3, 28),
            field("shape", I32, 31),
        ],
    }
}

/// A body as drawn: its pose at the frame's time (interpolated between
/// steps), its pose one step earlier, and its render fields.
pub fn rigid_layout() -> Layout {
    use FieldTy::*;
    Layout {
        name: "Rigid".into(),
        stride: RIGID_WORDS as u32,
        fields: vec![
            field("pos", Vec3, 0),
            field("rot", Vec4, 3),
            field("scale", Vec3, 7),
            field("color", Vec4, 10),
            field("seed", I32, 14),
            field("id", I32, 15),
            field("prev_pos", Vec3, 16),
            field("prev_rot", Vec4, 19),
            field("age", F32, 23),
            field("radius", F32, 24),
            field("speed", F32, 25),
            field("group", I32, 26),
        ],
    }
}

/// A hit between two bodies: where, along which normal, how hard, when.
pub fn contact_layout() -> Layout {
    use FieldTy::*;
    Layout {
        name: "Contact".into(),
        stride: CONTACT_WORDS as u32,
        fields: vec![
            field("pos", Vec3, 0),
            field("normal", Vec3, 3),
            field("impulse", F32, 6),
            field("speed", F32, 7),
            field("time", F32, 8),
            field("a", I32, 9),
            field("b", I32, 10),
            field("step", I32, 11),
        ],
    }
}

/// Something that happened to a body: spawned, expired, fell asleep.
pub fn event_layout() -> Layout {
    use FieldTy::*;
    Layout {
        name: "Event".into(),
        stride: EVENT_WORDS as u32,
        fields: vec![field("kind", I32, 0), field("id", I32, 1), field("time", F32, 2), field("step", I32, 3), field("pos", Vec3, 4)],
    }
}

/// Every physics layout, for a kernel compile.
pub fn layouts() -> Vec<Layout> {
    vec![body_layout(), rigid_layout(), contact_layout(), event_layout()]
}

/// A shape a `Body` record names (`shape` field); 0 takes the spawn's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RecordShape {
    Spawn,
    Box,
    Sphere,
    Capsule,
}

/// A decoded `Body` record.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BodyRecord {
    pub pos: [f32; 3],
    pub size: [f32; 3],
    pub rot: [f32; 4],
    pub vel: [f32; 3],
    pub spin: [f32; 3],
    pub drag: f32,
    pub spin_drag: f32,
    pub density: f32,
    pub friction: f32,
    pub bounce: f32,
    pub lift: f32,
    pub delay: f32,
    pub life: f32,
    pub color: [f32; 4],
    pub scale: [f32; 3],
    pub shape: i32,
}

fn f(w: &[u32], i: usize) -> f32 {
    f32::from_bits(w[i])
}

fn v3(w: &[u32], i: usize) -> [f32; 3] {
    [f(w, i), f(w, i + 1), f(w, i + 2)]
}

fn v4(w: &[u32], i: usize) -> [f32; 4] {
    [f(w, i), f(w, i + 1), f(w, i + 2), f(w, i + 3)]
}

/// Why a record was refused (the bridge creates nothing from it).
#[derive(Clone, Debug, PartialEq)]
pub struct RecordError {
    pub index: usize,
    pub field: &'static str,
    pub value: f32,
    pub allowed: &'static str,
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "body record {}: `{}` is {}, allowed {}", self.index, self.field, self.value, self.allowed)
    }
}

impl BodyRecord {
    pub fn from_words(w: &[u32]) -> Self {
        Self {
            pos: v3(w, 0),
            size: v3(w, 3),
            rot: v4(w, 6),
            vel: v3(w, 10),
            spin: v3(w, 13),
            drag: f(w, 16),
            spin_drag: f(w, 17),
            density: f(w, 18),
            friction: f(w, 19),
            bounce: f(w, 20),
            lift: f(w, 21),
            delay: f(w, 22),
            life: f(w, 23),
            color: v4(w, 24),
            scale: v3(w, 28),
            shape: w[31] as i32,
        }
    }

    pub fn to_words(&self, w: &mut [u32]) {
        let mut put = |i: usize, v: &[f32]| {
            for (k, x) in v.iter().enumerate() {
                w[i + k] = x.to_bits();
            }
        };
        put(0, &self.pos);
        put(3, &self.size);
        put(6, &self.rot);
        put(10, &self.vel);
        put(13, &self.spin);
        put(16, &[self.drag, self.spin_drag, self.density, self.friction, self.bounce, self.lift, self.delay, self.life]);
        put(24, &self.color);
        put(28, &self.scale);
        w[31] = self.shape as u32;
    }

    pub fn record_shape(&self) -> RecordShape {
        match self.shape {
            1 => RecordShape::Box,
            2 => RecordShape::Sphere,
            3 => RecordShape::Capsule,
            _ => RecordShape::Spawn,
        }
    }

    /// The caps every record passes before anything is created from it
    /// (KERNELS.md §3.6.3): finite values, sizes, speeds and material
    /// values in range. Sanity bounds, not budgets.
    pub fn validate(&self, index: usize) -> Result<(), RecordError> {
        let bad = |field: &'static str, value: f32, allowed: &'static str| Err(RecordError { index, field, value, allowed });
        let all = [
            ("pos", &self.pos[..], -1.0e5, 1.0e5, "-100000 .. 100000"),
            ("size", &self.size[..], 0.0, 1000.0, "0 .. 1000 (0 takes the spawn's size)"),
            ("rot", &self.rot[..], -1.0e3, 1.0e3, "a quaternion"),
            ("vel", &self.vel[..], -500.0, 500.0, "-500 .. 500 per axis"),
            ("spin", &self.spin[..], -1000.0, 1000.0, "-1000 .. 1000 rad/s per axis"),
            ("drag", &[self.drag][..], 0.0, 1000.0, "0 .. 1000"),
            ("spin_drag", &[self.spin_drag][..], 0.0, 1000.0, "0 .. 1000"),
            ("density", &[self.density][..], 0.0, 1.0e6, "0 .. 1000000 (0 is 1000)"),
            ("friction", &[self.friction][..], 0.0, 10.0, "0 .. 10 (0 is 0.6)"),
            ("bounce", &[self.bounce][..], 0.0, 1.0, "0 .. 1"),
            ("lift", &[self.lift][..], -10.0, 10.0, "-10 .. 10"),
            ("delay", &[self.delay][..], 0.0, 3600.0, "0 .. 3600 s"),
            ("life", &[self.life][..], 0.0, 3600.0, "0 .. 3600 s (0 lives forever)"),
            ("color", &self.color[..], -1.0e4, 1.0e4, "finite"),
            ("scale", &self.scale[..], 0.0, 1.0e4, "0 .. 10000 (0 is 1)"),
        ];
        for (name, vals, lo, hi, allowed) in all {
            for &v in vals {
                if !v.is_finite() || v < lo || v > hi {
                    return bad(name, v, allowed);
                }
            }
        }
        if !(0..=3).contains(&self.shape) {
            return bad("shape", self.shape as f32, "0 (the spawn's), 1 box, 2 sphere, 3 capsule");
        }
        Ok(())
    }

    /// The rotation, normalised; the identity when zero.
    pub fn rotation(&self) -> [f32; 4] {
        let q = self.rot;
        let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        if l < 1e-6 {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            [q[0] / l, q[1] / l, q[2] / l, q[3] / l]
        }
    }
}

/// An event's kind (`Event.kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum EventKind {
    Spawn = 0,
    Expire = 1,
    Sleep = 2,
}

/// A decoded contact.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContactRecord {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub impulse: f32,
    pub speed: f32,
    pub time: f32,
    pub a: u32,
    pub b: u32,
    pub step: u64,
}

impl ContactRecord {
    pub fn to_words(&self, w: &mut [u32]) {
        for k in 0..3 {
            w[k] = self.pos[k].to_bits();
            w[3 + k] = self.normal[k].to_bits();
        }
        w[6] = self.impulse.to_bits();
        w[7] = self.speed.to_bits();
        w[8] = self.time.to_bits();
        w[9] = self.a;
        w[10] = self.b;
        w[11] = self.step as u32;
    }
}

/// A decoded event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EventRecord {
    pub kind: EventKind,
    pub id: u32,
    pub time: f32,
    pub step: u64,
    pub pos: [f32; 3],
}

impl EventRecord {
    pub fn to_words(&self, w: &mut [u32]) {
        w[0] = self.kind as i32 as u32;
        w[1] = self.id;
        w[2] = self.time.to_bits();
        w[3] = self.step as u32;
        for k in 0..3 {
            w[4 + k] = self.pos[k].to_bits();
        }
        w[7] = 0;
    }
}

/// A body as drawn (one `Rigid` record).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RigidRecord {
    pub pos: [f32; 3],
    pub rot: [f32; 4],
    pub scale: [f32; 3],
    pub color: [f32; 4],
    pub seed: u32,
    pub id: u32,
    pub prev_pos: [f32; 3],
    pub prev_rot: [f32; 4],
    pub age: f32,
    pub radius: f32,
    pub speed: f32,
    pub group: u32,
}

impl RigidRecord {
    pub fn to_words(&self, w: &mut [u32]) {
        let mut put = |i: usize, v: &[f32]| {
            for (k, x) in v.iter().enumerate() {
                w[i + k] = x.to_bits();
            }
        };
        put(0, &self.pos);
        put(3, &self.rot);
        put(7, &self.scale);
        put(10, &self.color);
        put(16, &self.prev_pos);
        put(19, &self.prev_rot);
        put(23, &[self.age, self.radius, self.speed]);
        w[14] = self.seed;
        w[15] = self.id;
        w[26] = self.group;
        w[27] = 0;
    }

    /// Writes the fields `layout` names (by name and type) into one record
    /// of `layout.stride` words; the rest stay as they are (zero in a
    /// fresh slot). A draw shader's reflected instance record takes the
    /// fields it declares.
    pub fn write_as(&self, layout: &Layout, w: &mut [u32]) {
        for fl in &layout.fields {
            let o = fl.offset as usize;
            let vals: &[f32] = match (fl.name.as_str(), fl.ty) {
                ("pos", FieldTy::Vec3) => &self.pos,
                ("rot", FieldTy::Vec4) => &self.rot,
                ("scale", FieldTy::Vec3) => &self.scale,
                ("color", FieldTy::Vec4) => &self.color,
                ("prev_pos", FieldTy::Vec3) => &self.prev_pos,
                ("prev_rot", FieldTy::Vec4) => &self.prev_rot,
                ("age", FieldTy::F32) => std::slice::from_ref(&self.age),
                ("radius", FieldTy::F32) => std::slice::from_ref(&self.radius),
                ("speed", FieldTy::F32) => std::slice::from_ref(&self.speed),
                ("seed", FieldTy::I32) => {
                    w[o] = self.seed;
                    continue;
                }
                ("id", FieldTy::I32) => {
                    w[o] = self.id;
                    continue;
                }
                ("group", FieldTy::I32) => {
                    w[o] = self.group;
                    continue;
                }
                ("seed", FieldTy::F32) => {
                    w[o] = (self.seed as f32).to_bits();
                    continue;
                }
                ("id", FieldTy::F32) => {
                    w[o] = (self.id as f32).to_bits();
                    continue;
                }
                _ => continue,
            };
            for (k, x) in vals.iter().enumerate() {
                w[o + k] = x.to_bits();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_are_packed_without_overlap() {
        for l in layouts() {
            let mut used = vec![false; l.stride as usize];
            for fl in &l.fields {
                for k in 0..fl.ty.words() {
                    let i = (fl.offset + k) as usize;
                    assert!(!used[i], "{}.{} overlaps", l.name, fl.name);
                    used[i] = true;
                }
            }
        }
    }

    #[test]
    fn body_records_round_trip_and_validate() {
        let r = BodyRecord { pos: [1.0, 2.0, 3.0], size: [0.1, 0.2, 0.3], vel: [0.0, 5.0, 0.0], shape: 2, life: 3.0, ..Default::default() };
        let mut w = [0u32; BODY_WORDS];
        r.to_words(&mut w);
        assert_eq!(BodyRecord::from_words(&w), r);
        assert!(r.validate(0).is_ok());
        assert_eq!(r.rotation(), [0.0, 0.0, 0.0, 1.0]);
        let bad = BodyRecord { vel: [f32::NAN, 0.0, 0.0], ..r };
        assert_eq!(bad.validate(4).unwrap_err().field, "vel");
        let bad = BodyRecord { size: [0.1, -1.0, 0.1], ..r };
        assert_eq!(bad.validate(4).unwrap_err().field, "size");
        let bad = BodyRecord { shape: 9, ..r };
        assert_eq!(bad.validate(4).unwrap_err().field, "shape");
    }
}
