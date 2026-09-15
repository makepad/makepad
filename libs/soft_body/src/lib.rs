//! Bounded XPBD character deformation. Own this state on a worker; the
//! playable capsule remains authoritative and supplies an anchor pose.

pub const MAX_SOFT_PARTICLES: usize = 128;
pub const MAX_SOFT_TETRAHEDRA: usize = 128;
pub const MAX_SOFT_CONTACT_SAMPLES: usize = 128;
pub const MAX_SOFT_COLLIDERS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoftBodyBinding {
    pub tetrahedron: u16,
    pub weights: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct SoftBodyDefinition {
    pub rest_positions: Vec<[f32; 3]>,
    pub tetrahedra: Vec<[u16; 4]>,
    /// Embedded samples on the visible surface, not the enclosing cage.
    pub surface_samples: Vec<SoftBodyBinding>,
    /// Kinematic core particles; other particles have compliant pose targets.
    pub anchors: Vec<u16>,
    #[cfg(test)]
    validation_count: std::cell::Cell<usize>,
}

/// Validated immutable cage and its precomputed rest inverses. Reuse this
/// within a batch of embeddings; a point lookup neither allocates nor repeats
/// topology validation. The borrow prevents the cage changing under the cache.
#[derive(Debug)]
pub struct SoftBodyBinder<'a> {
    definition: &'a SoftBodyDefinition,
    inverse_rest: Vec<M3>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoftBodySettings {
    pub mass: f32,
    pub edge_compliance: f32,
    pub volume_compliance: f32,
    pub pose_compliance: f32,
    /// Velocity damping per second.
    pub damping: f32,
    pub gravity: [f32; 3],
    pub contact_radius: f32,
    pub friction: f32,
    pub substeps: u8,
    pub iterations: u8,
    pub max_speed: f32,
    pub max_displacement: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoftBodyPose {
    pub translation: [f32; 3],
    /// Normalized XYZW; scale is baked into the definition on a worker.
    pub rotation: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SoftBodyCollider {
    /// Free space has dot(normal, point) >= offset.
    Plane {
        normal: [f32; 3],
        offset: f32,
    },
    Sphere {
        center: [f32; 3],
        radius: f32,
    },
    Capsule {
        a: [f32; 3],
        b: [f32; 3],
        radius: f32,
    },
    Box {
        center: [f32; 3],
        half_extents: [f32; 3],
        rotation: [f32; 4],
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoftBodyAffine {
    /// Row-major affine transform from authored model space to deformed
    /// model space. Palette consumers must also transform normals correctly.
    pub matrix: [[f32; 4]; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SoftBodyStats {
    pub contacts: u32,
    pub max_edge_strain: f32,
    pub min_volume_ratio: f32,
    pub max_volume_error: f32,
    pub recovered: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoftBodyFrame {
    /// Model-local coordinates relative to the supplied anchor pose.
    pub positions: Vec<[f32; 3]>,
    /// Same order as SoftBodyDefinition::tetrahedra.
    pub tetrahedra: Vec<SoftBodyAffine>,
    pub bounds: [[f32; 3]; 2],
    pub stats: SoftBodyStats,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoftBodyAttachmentFrame {
    pub position: [f32; 3],
    /// Rigid rotation extracted from the cell deformation; no scale/shear.
    pub rotation: [f32; 4],
}

type V3 = [f32; 3];
type M3 = [[f32; 3]; 3];
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(a: V3) -> f32 {
    (dot(a, a)).sqrt()
}
fn unit(a: V3) -> V3 {
    let l = length(a);
    if l > 1e-12 {
        mul(a, 1. / l)
    } else {
        [0., 1., 0.]
    }
}
fn finite(a: V3) -> bool {
    a.iter().all(|v| v.is_finite())
}
fn columns(a: V3, b: V3, c: V3) -> M3 {
    [[a[0], b[0], c[0]], [a[1], b[1], c[1]], [a[2], b[2], c[2]]]
}
fn transpose(m: M3) -> M3 {
    columns(m[0], m[1], m[2])
}
fn mv(m: M3, v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}
fn mm(a: M3, b: M3) -> M3 {
    let b = transpose(b);
    std::array::from_fn(|r| std::array::from_fn(|c| dot(a[r], b[c])))
}
fn inverse(m: M3) -> Option<M3> {
    let c = [cross(m[1], m[2]), cross(m[2], m[0]), cross(m[0], m[1])];
    let det = dot(m[0], c[0]);
    if !det.is_finite() || det.abs() < 1e-18 {
        return None;
    }
    Some(transpose(c.map(|v| mul(v, 1. / det))))
}
fn tet_matrix(p: &[V3], t: [u16; 4]) -> M3 {
    let a = p[t[0] as usize];
    columns(
        sub(p[t[1] as usize], a),
        sub(p[t[2] as usize], a),
        sub(p[t[3] as usize], a),
    )
}
fn volume(p: &[V3], t: [u16; 4]) -> f32 {
    let a = p[t[0] as usize];
    dot(
        sub(p[t[1] as usize], a),
        cross(sub(p[t[2] as usize], a), sub(p[t[3] as usize], a)),
    ) / 6.
}
fn binding_point(p: &[V3], t: [u16; 4], w: [f32; 4]) -> V3 {
    let mut out = [0.; 3];
    for i in 0..4 {
        out = add(out, mul(p[t[i] as usize], w[i]));
    }
    out
}
fn binding_valid(binding: SoftBodyBinding, count: usize) -> bool {
    (binding.tetrahedron as usize) < count
        && binding
            .weights
            .iter()
            .all(|w| w.is_finite() && *w >= 0. && *w <= 1.)
        && (binding.weights.iter().sum::<f32>() - 1.).abs() < 1e-4
}
fn quaternion_valid(q: [f32; 4]) -> bool {
    q.iter().all(|v| v.is_finite()) && (q.iter().map(|v| v * v).sum::<f32>() - 1.).abs() < 1e-3
}
fn rotate(q: [f32; 4], v: V3) -> V3 {
    let u = [q[0], q[1], q[2]];
    let t = mul(cross(u, v), 2.);
    add(v, add(mul(t, q[3]), cross(u, t)))
}
fn conjugate(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

impl Default for SoftBodyPose {
    fn default() -> Self {
        Self {
            translation: [0.; 3],
            rotation: [0., 0., 0., 1.],
        }
    }
}
impl SoftBodyPose {
    fn validate(self) -> Result<(), String> {
        if !finite(self.translation)
            || self.translation.iter().any(|v| v.abs() > 1e6)
            || !quaternion_valid(self.rotation)
        {
            return Err("invalid soft-body anchor pose".into());
        }
        Ok(())
    }
    fn point(self, p: V3) -> V3 {
        add(self.translation, rotate(self.rotation, p))
    }
    fn local(self, p: V3) -> V3 {
        rotate(conjugate(self.rotation), sub(p, self.translation))
    }
    fn interpolate(self, b: Self, t: f32) -> Self {
        let sign = if self
            .rotation
            .iter()
            .zip(b.rotation)
            .map(|(a, b)| a * b)
            .sum::<f32>()
            < 0.
        {
            -1.
        } else {
            1.
        };
        let mut q = std::array::from_fn::<_, 4, _>(|i| {
            self.rotation[i] * (1. - t) + b.rotation[i] * t * sign
        });
        let len = (q.iter().map(|v| v * v).sum::<f32>()).sqrt();
        for v in &mut q {
            *v /= len;
        }
        Self {
            translation: add(mul(self.translation, 1. - t), mul(b.translation, t)),
            rotation: q,
        }
    }
}

impl Default for SoftBodySettings {
    fn default() -> Self {
        Self {
            mass: 1.,
            edge_compliance: 0.02,
            volume_compliance: 1e-7,
            pose_compliance: 0.02,
            damping: 4.,
            gravity: [0., -9.81, 0.],
            contact_radius: 0.01,
            friction: 0.5,
            substeps: 4,
            iterations: 6,
            max_speed: 20.,
            max_displacement: 2.,
        }
    }
}
impl SoftBodySettings {
    pub fn validate(&self) -> Result<(), String> {
        let range = |v: f32, min: f32, max: f32| v.is_finite() && v >= min && v <= max;
        if !range(self.mass, f32::MIN_POSITIVE, 10000.)
            || ![
                self.edge_compliance,
                self.volume_compliance,
                self.pose_compliance,
            ]
            .into_iter()
            .all(|v| range(v, 0., 1000.))
            || !range(self.damping, 0., 100.)
            || !finite(self.gravity)
            || self.gravity.iter().any(|v| v.abs() > 100.)
            || !range(self.contact_radius, 0., 10.)
            || !range(self.friction, 0., 2.)
            || !(1..=8).contains(&self.substeps)
            || !(1..=16).contains(&self.iterations)
            || !range(self.max_speed, f32::MIN_POSITIVE, 1000.)
            || !range(self.max_displacement, f32::MIN_POSITIVE, 1000.)
        {
            return Err("soft-body settings exceed finite physical budgets".into());
        }
        Ok(())
    }
}

impl SoftBodyDefinition {
    pub fn validate(&self) -> Result<(), String> {
        #[cfg(test)]
        self.validation_count.set(self.validation_count.get() + 1);
        if self.rest_positions.len() < 4
            || self.rest_positions.len() > MAX_SOFT_PARTICLES
            || self.tetrahedra.is_empty()
            || self.tetrahedra.len() > MAX_SOFT_TETRAHEDRA
            || self.surface_samples.len() > MAX_SOFT_CONTACT_SAMPLES
        {
            return Err("soft-body topology budget".into());
        }
        if self
            .rest_positions
            .iter()
            .any(|p| !finite(*p) || p.iter().any(|v| v.abs() > 10000.))
        {
            return Err("invalid soft-body rest coordinates".into());
        }
        let mut used = vec![false; self.rest_positions.len()];
        let mut cells = std::collections::BTreeSet::new();
        for &t in &self.tetrahedra {
            let mut sorted = t;
            sorted.sort();
            if sorted.iter().any(|i| *i as usize >= used.len())
                || sorted.windows(2).any(|p| p[0] == p[1])
                || !cells.insert(sorted)
            {
                return Err("invalid or duplicate soft-body tetrahedron".into());
            }
            let v = volume(&self.rest_positions, t);
            if !v.is_finite()
                || v <= 1e-12
                || inverse(tet_matrix(&self.rest_positions, t)).is_none()
            {
                return Err(
                    "soft-body tetrahedra require positive nondegenerate rest volume".into(),
                );
            }
            for i in t {
                used[i as usize] = true;
            }
        }
        if used.iter().any(|used| !*used) {
            return Err("soft-body particle has no cell".into());
        }
        let mut anchors = std::collections::BTreeSet::new();
        if self
            .anchors
            .iter()
            .any(|i| *i as usize >= used.len() || !anchors.insert(*i))
        {
            return Err("invalid or duplicate soft-body anchor".into());
        }
        if self
            .surface_samples
            .iter()
            .any(|b| !binding_valid(*b, self.tetrahedra.len()))
        {
            return Err("invalid soft-body surface binding".into());
        }
        Ok(())
    }

    /// Enclosing 43-particle/80-cell cage. The visible ellipsoid is never
    /// altered: only its control cage is expanded to enclose curved faces.
    pub fn ellipsoid(center: V3, radii: V3) -> Result<Self, String> {
        if !finite(center)
            || center.iter().any(|v| v.abs() > 1000.)
            || radii
                .iter()
                .any(|v| !v.is_finite() || *v < 0.001 || *v > 100.)
        {
            return Err("soft-body ellipsoid dimensions".into());
        }
        let g = 1.618_034_f32;
        let mut points = vec![
            [-1., g, 0.],
            [1., g, 0.],
            [-1., -g, 0.],
            [1., -g, 0.],
            [0., -1., g],
            [0., 1., g],
            [0., -1., -g],
            [0., 1., -g],
            [g, 0., -1.],
            [g, 0., 1.],
            [-g, 0., -1.],
            [-g, 0., 1.],
        ];
        for p in &mut points {
            *p = unit(*p);
        }
        let faces: [[u16; 3]; 20] = [
            [0, 11, 5],
            [0, 5, 1],
            [0, 1, 7],
            [0, 7, 10],
            [0, 10, 11],
            [1, 5, 9],
            [5, 11, 4],
            [11, 10, 2],
            [10, 7, 6],
            [7, 1, 8],
            [3, 9, 4],
            [3, 4, 2],
            [3, 2, 6],
            [3, 6, 8],
            [3, 8, 9],
            [4, 9, 5],
            [2, 4, 11],
            [6, 2, 10],
            [8, 6, 7],
            [9, 8, 1],
        ];
        let mut midpoints = std::collections::BTreeMap::new();
        let mut refined = Vec::with_capacity(80);
        for [a, b, c] in faces {
            let mut midpoint = |a: u16, b: u16| -> u16 {
                let edge = if a < b { (a, b) } else { (b, a) };
                if let Some(i) = midpoints.get(&edge) {
                    return *i;
                }
                let i = points.len() as u16;
                points.push(unit(add(points[a as usize], points[b as usize])));
                midpoints.insert(edge, i);
                i
            };
            let ab = midpoint(a, b);
            let bc = midpoint(b, c);
            let ca = midpoint(c, a);
            refined.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        let mut inradius = 1.0_f32;
        for &[a, b, c] in &refined {
            let a = points[a as usize];
            let n = unit(cross(
                sub(points[b as usize], a),
                sub(points[c as usize], a),
            ));
            inradius = inradius.min(dot(n, a).abs());
        }
        let inflate = (1. + 1e-4) / inradius;
        let mapped = |p: V3, scale: f32| {
            add(
                center,
                [
                    p[0] * radii[0] * scale,
                    p[1] * radii[1] * scale,
                    p[2] * radii[2] * scale,
                ],
            )
        };
        let surface = points.iter().map(|p| mapped(*p, 1.)).collect::<Vec<_>>();
        let mut rest_positions = points
            .iter()
            .map(|p| mapped(*p, inflate))
            .collect::<Vec<_>>();
        let core = rest_positions.len() as u16;
        rest_positions.push(center);
        let tetrahedra = refined
            .into_iter()
            .map(|[a, b, c]| {
                let mut tet = [core, a, b, c];
                if volume(&rest_positions, tet) < 0. {
                    tet.swap(2, 3);
                }
                tet
            })
            .collect();
        let mut definition = Self {
            rest_positions,
            tetrahedra,
            surface_samples: Vec::new(),
            anchors: vec![core],
            #[cfg(test)]
            validation_count: std::cell::Cell::new(0),
        };
        let binder = definition.binder()?;
        definition.surface_samples = surface
            .into_iter()
            .map(|p| binder.bind_point(p))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(definition)
    }

    /// Validate once and prepare a batch of point embeddings. Keep independent
    /// skin-weight checks at callers: geometry identity alone does not prove
    /// that subsequently edited weights still reference their enclosing cell.
    pub fn binder(&self) -> Result<SoftBodyBinder<'_>, String> {
        self.validate()?;
        Ok(SoftBodyBinder {
            definition: self,
            inverse_rest: self.tetrahedra.iter()
                .map(|t| inverse(tet_matrix(&self.rest_positions, *t)).unwrap())
                .collect(),
        })
    }

    pub fn bind_point(&self, rest_position: V3) -> Result<SoftBodyBinding, String> {
        self.binder()?.bind_point(rest_position)
    }
}

impl SoftBodyBinder<'_> {
    pub fn bind_point(&self, rest_position: V3) -> Result<SoftBodyBinding, String> {
        if !finite(rest_position) {
            return Err("non-finite soft-body binding point".into());
        }
        for (i, &t) in self.definition.tetrahedra.iter().enumerate() {
            let bary = mv(
                self.inverse_rest[i],
                sub(rest_position, self.definition.rest_positions[t[0] as usize]),
            );
            let mut w = [1. - bary[0] - bary[1] - bary[2], bary[0], bary[1], bary[2]];
            if w.iter().all(|v| *v >= -1e-5 && *v <= 1. + 1e-5) {
                for value in &mut w {
                    *value = value.max(0.);
                }
                let total = w.iter().sum::<f32>();
                for value in &mut w {
                    *value /= total;
                }
                return Ok(SoftBodyBinding {
                    tetrahedron: i as u16,
                    weights: w,
                });
            }
        }
        Err("point is outside the soft-body cage; enlarge the cage without altering visible geometry".into())
    }
}

#[derive(Clone, Debug)]
struct Edge {
    a: usize,
    b: usize,
    rest: f32,
}

/// Retained physical state. All mutation stays on its owning worker. No
/// locks, renderer handles, asset lookups or wall-clock time are involved.
#[derive(Clone, Debug)]
pub struct SoftBodyState {
    definition: std::sync::Arc<SoftBodyDefinition>,
    settings: SoftBodySettings,
    pose: SoftBodyPose,
    positions: Vec<V3>,
    previous: Vec<V3>,
    velocities: Vec<V3>,
    inverse_mass: Vec<f32>,
    rest_volumes: Vec<f32>,
    inverse_rest: Vec<M3>,
    edges: Vec<Edge>,
    edge_lambda: Vec<f32>,
    volume_lambda: Vec<f32>,
    pose_lambda: Vec<V3>,
    stats: SoftBodyStats,
}

impl SoftBodyState {
    pub fn new(
        definition: std::sync::Arc<SoftBodyDefinition>,
        settings: SoftBodySettings,
        pose: SoftBodyPose,
    ) -> Result<Self, String> {
        definition.validate()?;
        settings.validate()?;
        pose.validate()?;
        let rest_volumes = definition
            .tetrahedra
            .iter()
            .map(|t| volume(&definition.rest_positions, *t))
            .collect::<Vec<_>>();
        let total = rest_volumes.iter().sum::<f32>();
        let mut masses = vec![0.; definition.rest_positions.len()];
        for (t, v) in definition.tetrahedra.iter().zip(&rest_volumes) {
            for i in t {
                masses[*i as usize] += v * 0.25 * settings.mass / total;
            }
        }
        let mut inverse_mass = masses.iter().map(|mass| 1. / mass).collect::<Vec<_>>();
        if inverse_mass.iter().any(|mass| !mass.is_finite()) {
            return Err("soft-body mass underflows particle precision".into());
        }
        for &i in &definition.anchors {
            inverse_mass[i as usize] = 0.;
        }
        let mut edge_ids = std::collections::BTreeSet::new();
        for t in &definition.tetrahedra {
            for a in 0..4 {
                for b in a + 1..4 {
                    edge_ids.insert((t[a].min(t[b]) as usize, t[a].max(t[b]) as usize));
                }
            }
        }
        let edges = edge_ids
            .into_iter()
            .map(|(a, b)| Edge {
                a,
                b,
                rest: length(sub(
                    definition.rest_positions[a],
                    definition.rest_positions[b],
                )),
            })
            .collect::<Vec<_>>();
        let positions = definition
            .rest_positions
            .iter()
            .map(|p| pose.point(*p))
            .collect::<Vec<_>>();
        let inverse_rest = definition
            .tetrahedra
            .iter()
            .map(|t| inverse(tet_matrix(&definition.rest_positions, *t)).unwrap())
            .collect();
        let state = Self {
            edge_lambda: vec![0.; edges.len()],
            volume_lambda: vec![0.; rest_volumes.len()],
            pose_lambda: vec![[0.; 3]; positions.len()],
            previous: positions.clone(),
            velocities: vec![[0.; 3]; positions.len()],
            positions,
            definition,
            settings,
            pose,
            inverse_mass,
            rest_volumes,
            inverse_rest,
            edges,
            stats: SoftBodyStats {
                min_volume_ratio: 1.,
                ..Default::default()
            },
        };
        Ok(state)
    }

    pub fn reset(&mut self, pose: SoftBodyPose) -> Result<(), String> {
        pose.validate()?;
        self.pose = pose;
        for (p, rest) in self
            .positions
            .iter_mut()
            .zip(&self.definition.rest_positions)
        {
            *p = pose.point(*rest);
        }
        self.previous.clone_from(&self.positions);
        self.velocities.fill([0.; 3]);
        self.clear_lambdas();
        self.stats = SoftBodyStats {
            min_volume_ratio: 1.,
            ..Default::default()
        };
        Ok(())
    }

    /// Carry the existing deformation into a new drive orientation without
    /// injecting angular inertia. Character controllers use this for changes
    /// of facing; translation, gravity and contacts still go through `step`.
    /// Unlike `reset`, this preserves both deformation and its velocity.
    pub fn transport_rotation(&mut self, rotation: [f32; 4]) -> Result<(), String> {
        let next = SoftBodyPose { rotation, ..self.pose };
        next.validate()?;
        if rotation == self.pose.rotation { return Ok(()); }
        let old = self.pose;
        for point in self.positions.iter_mut().chain(&mut self.previous) {
            *point = next.point(old.local(*point));
        }
        for velocity in &mut self.velocities {
            *velocity = rotate(rotation, rotate(conjugate(old.rotation), *velocity));
        }
        self.pose = next;
        self.clear_lambdas();
        Ok(())
    }

    /// An impulse is kg*m/s in world axes, applied through the same embedded
    /// point used for collision. Pinned core particles absorb their share.
    pub fn apply_impulse(&mut self, binding: SoftBodyBinding, impulse: V3) -> Result<(), String> {
        if !binding_valid(binding, self.definition.tetrahedra.len())
            || !finite(impulse)
            || impulse.iter().any(|v| v.abs() > 10000.)
        {
            return Err("invalid soft-body impulse".into());
        }
        let t = self.definition.tetrahedra[binding.tetrahedron as usize];
        let magnitude = length(impulse) as f64;
        for i in 0..4 {
            let p = t[i] as usize;
            // Cap the velocity contribution before narrowing to f32. A
            // finite extreme inverse mass must not overflow impulse math.
            let factor = (binding.weights[i] as f64 * self.inverse_mass[p] as f64)
                .min(2. * self.settings.max_speed as f64 / magnitude.max(1e-30))
                as f32;
            self.velocities[p] = limit(
                add(self.velocities[p], mul(impulse, factor)),
                self.settings.max_speed,
            );
        }
        Ok(())
    }

    /// Advance a caller-scheduled fixed tick (at most 1/30 second). This
    /// function never catches up unbounded elapsed wall time. Bad input is
    /// refused before any state changes. Normal output is relative to drive.
    pub fn step(
        &mut self,
        dt: f32,
        drive: SoftBodyPose,
        colliders: &[SoftBodyCollider],
    ) -> Result<SoftBodyStats, String> {
        if !dt.is_finite() || dt <= 0. || dt > 1. / 30. + 1e-7 {
            return Err("soft-body dt must be positive and at most 1/30 second".into());
        }
        drive.validate()?;
        if colliders.len() > MAX_SOFT_COLLIDERS {
            return Err("soft-body contact proxy budget".into());
        }
        for collider in colliders {
            collider.validate()?;
        }
        let h = dt / self.settings.substeps as f32;
        let old_pose = self.pose;
        self.stats = SoftBodyStats {
            min_volume_ratio: 1.,
            ..Default::default()
        };
        for substep in 0..self.settings.substeps {
            let anchor =
                old_pose.interpolate(drive, (substep + 1) as f32 / self.settings.substeps as f32);
            self.previous.clone_from(&self.positions);
            self.clear_lambdas();
            for i in 0..self.positions.len() {
                if self.inverse_mass[i] == 0. {
                    self.positions[i] = anchor.point(self.definition.rest_positions[i]);
                    continue;
                }
                self.velocities[i] = limit(
                    mul(
                        add(self.velocities[i], mul(self.settings.gravity, h)),
                        1. / (1. + self.settings.damping * h),
                    ),
                    self.settings.max_speed,
                );
                self.positions[i] = add(self.positions[i], mul(self.velocities[i], h));
            }
            for _ in 0..self.settings.iterations {
                self.solve_edges(h);
                self.solve_volumes(h);
                self.solve_pose(h, anchor);
                self.solve_contacts(colliders);
            }
            let mut valid = true;
            for i in 0..self.positions.len() {
                let target = anchor.point(self.definition.rest_positions[i]);
                if !finite(self.positions[i]) {
                    valid = false;
                    break;
                }
                // A bounded leash is a numerical guard, not the deformation
                // model. Ordinary motion is solved by compliant targets.
                self.positions[i] = add(
                    target,
                    limit(
                        sub(self.positions[i], target),
                        self.settings.max_displacement,
                    ),
                );
                self.velocities[i] = if self.inverse_mass[i] == 0. {
                    [0.; 3]
                } else {
                    limit(
                        mul(sub(self.positions[i], self.previous[i]), 1. / h),
                        self.settings.max_speed,
                    )
                };
            }
            for (t, rest) in self.definition.tetrahedra.iter().zip(&self.rest_volumes) {
                let ratio = volume(&self.positions, *t) / rest;
                if !ratio.is_finite() || ratio < 0.05 {
                    valid = false;
                    break;
                }
                self.stats.min_volume_ratio = self.stats.min_volume_ratio.min(ratio);
                self.stats.max_volume_error = self.stats.max_volume_error.max((ratio - 1.).abs());
            }
            if !valid {
                let contacts = self.stats.contacts;
                self.reset(drive)?;
                self.stats.contacts = contacts;
                self.stats.recovered = true;
                return Ok(self.stats);
            }
        }
        self.pose = drive;
        for edge in &self.edges {
            self.stats.max_edge_strain = self.stats.max_edge_strain.max(
                (length(sub(self.positions[edge.a], self.positions[edge.b])) / edge.rest - 1.)
                    .abs(),
            );
        }
        Ok(self.stats)
    }

    fn clear_lambdas(&mut self) {
        self.edge_lambda.fill(0.);
        self.volume_lambda.fill(0.);
        self.pose_lambda.fill([0.; 3]);
    }

    fn solve_edges(&mut self, h: f32) {
        let alpha = self.settings.edge_compliance / (h * h);
        for (i, edge) in self.edges.iter().enumerate() {
            let d = sub(self.positions[edge.a], self.positions[edge.b]);
            let len = length(d);
            let mass = self.inverse_mass[edge.a] + self.inverse_mass[edge.b];
            if len < 1e-12 || mass == 0. {
                continue;
            }
            let delta = (-(len - edge.rest) - alpha * self.edge_lambda[i]) / (mass + alpha);
            self.edge_lambda[i] += delta;
            let n = mul(d, 1. / len);
            self.positions[edge.a] = add(
                self.positions[edge.a],
                mul(n, delta * self.inverse_mass[edge.a]),
            );
            self.positions[edge.b] = sub(
                self.positions[edge.b],
                mul(n, delta * self.inverse_mass[edge.b]),
            );
        }
    }

    fn solve_volumes(&mut self, h: f32) {
        let alpha = self.settings.volume_compliance / (h * h);
        for (cell, &t) in self.definition.tetrahedra.iter().enumerate() {
            let a = self.positions[t[0] as usize];
            let b = sub(self.positions[t[1] as usize], a);
            let c = sub(self.positions[t[2] as usize], a);
            let d = sub(self.positions[t[3] as usize], a);
            let gb = mul(cross(c, d), 1. / 6.);
            let gc = mul(cross(d, b), 1. / 6.);
            let gd = mul(cross(b, c), 1. / 6.);
            let ga = mul(add(add(gb, gc), gd), -1.);
            let gradients = [ga, gb, gc, gd];
            let mut mass = 0.;
            for i in 0..4 {
                mass += self.inverse_mass[t[i] as usize] * dot(gradients[i], gradients[i]);
            }
            if mass < 1e-20 {
                continue;
            }
            let constraint = dot(b, cross(c, d)) / 6. - self.rest_volumes[cell];
            let delta = (-constraint - alpha * self.volume_lambda[cell]) / (mass + alpha);
            self.volume_lambda[cell] += delta;
            for i in 0..4 {
                let p = t[i] as usize;
                self.positions[p] = add(
                    self.positions[p],
                    mul(gradients[i], delta * self.inverse_mass[p]),
                );
            }
        }
    }

    fn solve_pose(&mut self, h: f32, pose: SoftBodyPose) {
        let alpha = self.settings.pose_compliance / (h * h);
        for i in 0..self.positions.len() {
            if self.inverse_mass[i] == 0. {
                continue;
            }
            let target = pose.point(self.definition.rest_positions[i]);
            for d in 0..3 {
                let delta = (-(self.positions[i][d] - target[d]) - alpha * self.pose_lambda[i][d])
                    / (self.inverse_mass[i] + alpha);
                self.pose_lambda[i][d] += delta;
                self.positions[i][d] += delta * self.inverse_mass[i];
            }
        }
    }

    fn solve_contacts(&mut self, colliders: &[SoftBodyCollider]) {
        for sample in &self.definition.surface_samples {
            let t = self.definition.tetrahedra[sample.tetrahedron as usize];
            let w = sample.weights;
            let previous = binding_point(&self.previous, t, w);
            let mass = (0..4)
                .map(|i| self.inverse_mass[t[i] as usize] * w[i] * w[i])
                .sum::<f32>();
            if mass < 1e-20 {
                continue;
            }
            for collider in colliders {
                let point = binding_point(&self.positions, t, w);
                if let Some((normal, depth)) =
                    collider.contact(previous, point, self.settings.contact_radius)
                {
                    let motion = sub(point, previous);
                    let tangent = sub(motion, mul(normal, dot(motion, normal)));
                    let correction = sub(
                        mul(normal, depth),
                        limit(tangent, self.settings.friction * depth),
                    );
                    for i in 0..4 {
                        let p = t[i] as usize;
                        self.positions[p] = add(
                            self.positions[p],
                            mul(correction, self.inverse_mass[p] * w[i] / mass),
                        );
                    }
                    self.stats.contacts += 1;
                }
            }
        }
    }

    pub fn write_frame(&self, out: &mut SoftBodyFrame) {
        out.positions.clear();
        out.positions
            .extend(self.positions.iter().map(|p| self.pose.local(*p)));
        out.tetrahedra.clear();
        for (i, &t) in self.definition.tetrahedra.iter().enumerate() {
            let f = mm(tet_matrix(&out.positions, t), self.inverse_rest[i]);
            let translation = sub(
                out.positions[t[0] as usize],
                mv(f, self.definition.rest_positions[t[0] as usize]),
            );
            out.tetrahedra.push(SoftBodyAffine {
                matrix: std::array::from_fn(|r| [f[r][0], f[r][1], f[r][2], translation[r]]),
            });
        }
        out.bounds = [[f32::INFINITY; 3], [f32::NEG_INFINITY; 3]];
        for p in &out.positions {
            for d in 0..3 {
                out.bounds[0][d] = out.bounds[0][d].min(p[d]);
                out.bounds[1][d] = out.bounds[1][d].max(p[d]);
            }
        }
        out.stats = self.stats;
    }
}

fn limit(v: V3, max: f32) -> V3 {
    let len = length(v);
    if len > max {
        mul(v, max / len)
    } else {
        v
    }
}

impl SoftBodyCollider {
    pub fn validate(&self) -> Result<(), String> {
        let position = |p: V3| finite(p) && p.iter().all(|v| v.abs() <= 1e6);
        let radius = |r: f32| r.is_finite() && r > 0. && r <= 10000.;
        let valid = match *self {
            Self::Plane { normal, offset } => {
                finite(normal)
                    && (dot(normal, normal) - 1.).abs() < 1e-3
                    && offset.is_finite()
                    && offset.abs() <= 1e6
            }
            Self::Sphere { center, radius: r } => position(center) && radius(r),
            Self::Capsule { a, b, radius: r } => position(a) && position(b) && radius(r),
            Self::Box {
                center,
                half_extents,
                rotation,
            } => {
                position(center)
                    && half_extents.into_iter().all(radius)
                    && quaternion_valid(rotation)
            }
        };
        if valid {
            Ok(())
        } else {
            Err("invalid soft-body collision proxy".into())
        }
    }
    fn distance(&self, p: V3) -> (f32, V3) {
        match *self {
            Self::Plane { normal, offset } => (dot(p, normal) - offset, normal),
            Self::Sphere { center, radius } => {
                let d = sub(p, center);
                (length(d) - radius, unit(d))
            }
            Self::Capsule { a, b, radius } => {
                let ab = sub(b, a);
                let length2 = dot(ab, ab);
                let t = if length2 > 1e-20 {
                    (dot(sub(p, a), ab) / length2).clamp(0., 1.)
                } else {
                    0.
                };
                let d = sub(p, add(a, mul(ab, t)));
                (length(d) - radius, unit(d))
            }
            Self::Box {
                center,
                half_extents,
                rotation,
            } => {
                let local = rotate(conjugate(rotation), sub(p, center));
                let closest =
                    std::array::from_fn(|i| local[i].clamp(-half_extents[i], half_extents[i]));
                let d = sub(local, closest);
                let len = length(d);
                if len > 1e-12 {
                    (len, rotate(rotation, mul(d, 1. / len)))
                } else {
                    let mut axis = 0;
                    let mut gap = half_extents[0] - local[0].abs();
                    for i in 1..3 {
                        let candidate = half_extents[i] - local[i].abs();
                        if candidate < gap {
                            gap = candidate;
                            axis = i;
                        }
                    }
                    let mut n = [0.; 3];
                    n[axis] = if local[axis] < 0. { -1. } else { 1. };
                    (-gap, rotate(rotation, n))
                }
            }
        }
    }
    fn contact(&self, previous: V3, current: V3, radius: f32) -> Option<(V3, f32)> {
        let (old_distance, _) = self.distance(previous);
        let travel = sub(current, previous);
        let speed = length(travel);
        // Conservative advancement against convex signed-distance proxies
        // catches fast crossings that end outside a thin obstacle. This is
        // bounded to 24 distance evaluations per contact candidate.
        if old_distance > radius + 1e-5 && speed > 1e-8 {
            let mut fraction = 0.;
            for _ in 0..24 {
                let p = add(previous, mul(travel, fraction));
                let (distance, normal) = self.distance(p);
                let gap = distance - radius;
                if gap <= 1e-5 {
                    let depth = -dot(sub(current, p), normal) + 1e-5;
                    if depth > 0. {
                        return Some((normal, depth));
                    }
                    break;
                }
                fraction += gap / speed;
                if fraction > 1. {
                    break;
                }
            }
        }
        let (distance, normal) = self.distance(current);
        let depth = radius - distance;
        (depth > 0.).then_some((normal, depth))
    }
}

impl SoftBodyAffine {
    pub fn transform_point(&self, p: V3) -> V3 {
        std::array::from_fn(|r| {
            self.matrix[r][0] * p[0]
                + self.matrix[r][1] * p[1]
                + self.matrix[r][2] * p[2]
                + self.matrix[r][3]
        })
    }
    pub fn transform_normal(&self, n: V3) -> Option<V3> {
        let f = std::array::from_fn(|r| [self.matrix[r][0], self.matrix[r][1], self.matrix[r][2]]);
        let transformed = mv(transpose(inverse(f)?), n);
        (finite(transformed) && length(transformed) > 1e-12).then(|| unit(transformed))
    }
}

impl SoftBodyFrame {
    pub fn attachment(
        &self,
        definition: &SoftBodyDefinition,
        binding: SoftBodyBinding,
    ) -> Result<SoftBodyAttachmentFrame, String> {
        if self.positions.len() != definition.rest_positions.len()
            || self.tetrahedra.len() != definition.tetrahedra.len()
            || !binding_valid(binding, definition.tetrahedra.len())
        {
            return Err("soft-body attachment/frame mismatch".into());
        }
        let t = definition.tetrahedra[binding.tetrahedron as usize];
        if t.iter().any(|i| *i as usize >= self.positions.len()) {
            return Err("soft-body attachment cell indices".into());
        }
        let position = binding_point(&self.positions, t, binding.weights);
        let matrix = self.tetrahedra[binding.tetrahedron as usize].matrix;
        let mut rotation = std::array::from_fn(|r| [matrix[r][0], matrix[r][1], matrix[r][2]]);
        let det = dot(rotation[0], cross(rotation[1], rotation[2]));
        if !finite(position) || !det.is_finite() || det <= 1e-8 {
            return Err("soft-body attachment requires positive cell deformation".into());
        }
        // Newton polar decomposition followed by an orthonormal cleanup.
        // The result preserves gaze/eyeball geometry under body shear.
        for _ in 0..10 {
            let inverse_t = transpose(inverse(rotation).ok_or("singular soft-body attachment")?);
            rotation = std::array::from_fn(|r| {
                std::array::from_fn(|c| 0.5 * (rotation[r][c] + inverse_t[r][c]))
            });
        }
        let cols = transpose(rotation);
        let x = unit(cols[0]);
        let y = unit(sub(cols[1], mul(x, dot(cols[1], x))));
        let z = unit(cross(x, y));
        Ok(SoftBodyAttachmentFrame {
            position,
            rotation: matrix_quaternion(columns(x, y, z)),
        })
    }
}

fn matrix_quaternion(r: M3) -> [f32; 4] {
    let trace = r[0][0] + r[1][1] + r[2][2];
    let mut q = if trace > 0. {
        let s = (trace + 1.).sqrt() * 2.;
        [
            (r[2][1] - r[1][2]) / s,
            (r[0][2] - r[2][0]) / s,
            (r[1][0] - r[0][1]) / s,
            s * 0.25,
        ]
    } else {
        let i = if r[0][0] > r[1][1] && r[0][0] > r[2][2] {
            0
        } else if r[1][1] > r[2][2] {
            1
        } else {
            2
        };
        let j = (i + 1) % 3;
        let k = (i + 2) % 3;
        let s = ((1. + r[i][i] - r[j][j] - r[k][k]).max(0.)).sqrt() * 2.;
        let mut q = [0.; 4];
        q[i] = s * 0.25;
        q[j] = (r[j][i] + r[i][j]) / s;
        q[k] = (r[k][i] + r[i][k]) / s;
        q[3] = (r[k][j] - r[j][k]) / s;
        q
    };
    let len = (q.iter().map(|v| v * v).sum::<f32>()).sqrt();
    for v in &mut q {
        *v /= len;
    }
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_embedding_validates_once_and_reuses_rest_inverses() {
        let definition = SoftBodyDefinition::ellipsoid([0.1, 0.4, -0.2], [0.5, 0.8, 0.3]).unwrap();
        definition.validation_count.set(0);
        let binder = definition.binder().unwrap();
        for i in 0..20_000 {
            let angle = i as f32 * 0.1;
            let p = [0.1 + angle.cos() * 0.4, 0.4 + angle.sin() * 0.6, -0.2];
            let binding = binder.bind_point(p).unwrap();
            let tet = definition.tetrahedra[binding.tetrahedron as usize];
            let rebuilt = std::array::from_fn::<_, 3, _>(|axis| tet.iter().zip(binding.weights)
                .map(|(index, weight)| definition.rest_positions[*index as usize][axis] * weight).sum::<f32>());
            assert!(length(sub(rebuilt, p)) < 2e-5);
        }
        assert!(binder.bind_point([f32::NAN, 0., 0.]).is_err());
        assert!(binder.bind_point([5., 0., 0.]).is_err());
        assert_eq!(definition.validation_count.get(), 1);
        let mut invalid = definition.clone();
        invalid.tetrahedra[0].swap(0, 1);
        assert!(invalid.binder().is_err());
    }

    #[test]
    fn convex_sweep_catches_fast_crossings_that_end_outside() {
        let angle = std::f32::consts::FRAC_1_SQRT_2;
        for collider in [
            SoftBodyCollider::Sphere {
                center: [0.; 3],
                radius: 0.2,
            },
            SoftBodyCollider::Capsule {
                a: [0., -0.5, 0.],
                b: [0., 0.5, 0.],
                radius: 0.2,
            },
            SoftBodyCollider::Box {
                center: [0.; 3],
                half_extents: [0.005, 1., 1.],
                rotation: [0., 0., 0., 1.],
            },
            SoftBodyCollider::Box {
                center: [0.; 3],
                half_extents: [1., 1., 0.005],
                rotation: [0., angle, 0., angle],
            },
        ] {
            assert!(
                collider.distance([-2., 0., 0.]).0 > 0. && collider.distance([2., 0., 0.]).0 > 0.
            );
            let (n, depth) = collider
                .contact([-2., 0., 0.], [2., 0., 0.], 0.05)
                .expect("tunneled through convex obstacle");
            let corrected = add([2., 0., 0.], mul(n, depth));
            assert!(
                corrected[0] < 0.,
                "contact must stop on the entering side: {collider:?}"
            );
            assert!(collider.distance(corrected).0 >= 0.05 - 2e-5);
        }
    }
}
