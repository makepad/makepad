use makepad_csg_exact::Dyadic;
use makepad_csg_math::{orient2d, orient3d, Vec3d};
use std::cmp::Ordering;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

pub(crate) type ClassId = u32;

pub(crate) struct ClassPlane {
    seed: [Vec3d; 3],
    normal: [Dyadic; 3],
    offset: Dyadic,
    drop_axis: usize,
}

impl ClassPlane {
    pub(crate) fn seed(&self) -> [Vec3d; 3] {
        self.seed
    }

    pub(crate) fn drop_axis(&self) -> usize {
        self.drop_axis
    }
}

pub(crate) struct Virtual {
    /// Every class the point lies on: the defining ones and those it was found exactly on.
    classes: Vec<ClassId>,
    /// The classes the point was placed on (at most three): its coordinates solve these.
    defining: Vec<ClassId>,
    coordinates: [Dyadic; 4],
    approx: Vec3d,
    delta: f64,
    /// The solved coordinates are exactly the input position: predicates may read the input coordinates.
    at_input: bool,
}

impl Virtual {
    pub(crate) fn delta(&self) -> f64 {
        self.delta
    }
}

pub(crate) struct VirtualPoints {
    real: Vec<Vec3d>,
    classes: Vec<ClassPlane>,
    points: Vec<Option<Box<Virtual>>>,
    exact_evaluations: AtomicU64,
}

impl VirtualPoints {
    pub(crate) fn new(real: Vec<Vec3d>) -> VirtualPoints {
        let points = (0..real.len()).map(|_| None).collect();
        VirtualPoints {
            real,
            classes: Vec::new(),
            points,
            exact_evaluations: AtomicU64::new(0),
        }
    }

    pub(crate) fn add_class(&mut self, seed: [Vec3d; 3]) -> Option<ClassId> {
        let u = [0, 1, 2].map(|axis| {
            Dyadic::from_f64(seed[1].component(axis)).sub(&Dyadic::from_f64(seed[0].component(axis)))
        });
        let v = [0, 1, 2].map(|axis| {
            Dyadic::from_f64(seed[2].component(axis)).sub(&Dyadic::from_f64(seed[0].component(axis)))
        });
        let normal = cross(u.each_ref(), v.each_ref());
        if normal.iter().all(Dyadic::is_zero) {
            return None;
        }
        let origin = exact_vector(seed[0]);
        let offset = dot(normal.each_ref(), origin.each_ref());
        let drop_axis = dominant_axis(&normal);
        let id = self.classes.len() as ClassId;
        self.classes.push(ClassPlane {
            seed,
            normal,
            offset,
            drop_axis,
        });
        Some(id)
    }

    pub(crate) fn class(&self, c: ClassId) -> &ClassPlane {
        &self.classes[c as usize]
    }

    pub(crate) fn real(&self, v: u32) -> Vec3d {
        self.real[v as usize]
    }

    pub(crate) fn place(&self, v: u32, classes: &[ClassId]) -> Option<Virtual> {
        if classes.is_empty() || classes.len() > 3 {
            return None;
        }
        let mut classes = classes.to_vec();
        classes.sort_unstable();
        classes.dedup();
        let real = self.real(v);
        let input = exact_vector(real);
        let mut coordinates = match classes.as_slice() {
            &[a] => {
                let plane = self.class(a);
                let k = plane.drop_axis();
                let weight = plane.normal[k].clone();
                let mut coordinates = std::array::from_fn(|_| Dyadic::zero());
                let mut dropped = plane.offset.clone();
                for axis in 0..3 {
                    if axis != k {
                        coordinates[axis] = input[axis].mul(&weight);
                        dropped = dropped.sub(&plane.normal[axis].mul(&input[axis]));
                    }
                }
                coordinates[k] = dropped;
                coordinates[3] = weight;
                coordinates
            }
            &[a, b] => {
                let a = self.class(a);
                let b = self.class(b);
                let direction = cross(a.normal.each_ref(), b.normal.each_ref());
                let j = dominant_axis(&direction);
                if direction[j].is_zero() {
                    return None;
                }
                let [p, q] = match j {
                    0 => [1, 2],
                    1 => [0, 2],
                    _ => [0, 1],
                };
                let r1 = a.offset.sub(&a.normal[j].mul(&input[j]));
                let r2 = b.offset.sub(&b.normal[j].mul(&input[j]));
                let weight = a.normal[p].mul(&b.normal[q])
                    .sub(&a.normal[q].mul(&b.normal[p]));
                let mut coordinates = std::array::from_fn(|_| Dyadic::zero());
                coordinates[j] = input[j].mul(&weight);
                coordinates[p] = r1.mul(&b.normal[q])
                    .sub(&r2.mul(&a.normal[q]));
                coordinates[q] = a.normal[p].mul(&r2)
                    .sub(&b.normal[p].mul(&r1));
                coordinates[3] = weight;
                coordinates
            }
            &[a, b, c] => {
                let a = self.class(a);
                let b = self.class(b);
                let c = self.class(c);
                let bc = cross(b.normal.each_ref(), c.normal.each_ref());
                let ca = cross(c.normal.each_ref(), a.normal.each_ref());
                let ab = cross(a.normal.each_ref(), b.normal.each_ref());
                let weight = dot(a.normal.each_ref(), bc.each_ref());
                if weight.is_zero() {
                    return None;
                }
                let mut coordinates = std::array::from_fn(|_| Dyadic::zero());
                for axis in 0..3 {
                    coordinates[axis] = a.offset.mul(&bc[axis])
                        .add(&b.offset.mul(&ca[axis]))
                        .add(&c.offset.mul(&ab[axis]));
                }
                coordinates[3] = weight;
                coordinates
            }
            _ => return None,
        };
        if coordinates[3].sign() < 0 {
            coordinates = coordinates.map(|coordinate| coordinate.negate());
        }
        let approx = Vec3d::new(
            coordinates[0].ratio(&coordinates[3]),
            coordinates[1].ratio(&coordinates[3]),
            coordinates[2].ratio(&coordinates[3]),
        );
        let mut distance_squared = 0.0;
        let mut at_input = true;
        for axis in 0..3 {
            let numerator = coordinates[axis]
                .sub(&input[axis].mul(&coordinates[3]));
            at_input &= numerator.is_zero();
            let displacement = numerator.ratio(&coordinates[3]);
            distance_squared += displacement * displacement;
        }
        Some(Virtual {
            defining: classes.clone(),
            classes,
            coordinates,
            approx,
            delta: distance_squared.sqrt() * (1.0 + 1e-9),
            at_input,
        })
    }

    pub(crate) fn set(&mut self, v: u32, virt: Option<Virtual>) {
        self.points[v as usize] = virt.map(Box::new);
    }

    pub(crate) fn classes_of(&self, v: u32) -> &[ClassId] {
        self.points[v as usize]
            .as_ref()
            .map_or(&[], |point| point.classes.as_slice())
    }

    /// The classes a point was placed on (a subset of `classes_of`): two points with the same three
    /// are one point.
    pub(crate) fn defining_of(&self, v: u32) -> &[ClassId] {
        self.points[v as usize]
            .as_ref()
            .map_or(&[], |point| point.defining.as_slice())
    }

    /// Whether two classes' exact normals are parallel.
    pub(crate) fn parallel(&self, a: ClassId, b: ClassId) -> bool {
        cross(self.class(a).normal.each_ref(), self.class(b).normal.each_ref())
            .iter()
            .all(Dyadic::is_zero)
    }

    /// A placed point that lies exactly on `class` joins it without moving.
    pub(crate) fn add_membership(&mut self, v: u32, class: ClassId) {
        let point = self.points[v as usize].as_mut().expect("a placed point");
        if let Err(at) = point.classes.binary_search(&class) {
            point.classes.insert(at, class);
        }
    }

    pub(crate) fn approx(&self, v: u32) -> Vec3d {
        self.points[v as usize]
            .as_ref()
            .map_or(self.real(v), |point| point.approx)
    }

    pub(crate) fn displacement(&self, v: u32) -> f64 {
        self.points[v as usize].as_ref().map_or(0.0, |point| point.delta)
    }

    pub(crate) fn orient3d(&self, a: u32, b: u32, c: u32, d: u32) -> i32 {
        self.orient3d_tier(a, b, c, d).0
    }

    pub(crate) fn orient3d_tier(&self, a: u32, b: u32, c: u32, d: u32) -> (i32, u8) {
        let ids = [a, b, c, d];
        let sets = ids.map(|id| self.classes_of(id));
        if ids.iter().all(|&id| self.at_input(id)) {
            return (sign(orient3d(self.real(a), self.real(b), self.real(c), self.real(d))), 0);
        }
        let defining = ids.map(|id| self.defining_of(id));
        for i in 0..4 {
            for j in i + 1..4 {
                if ids[i] == ids[j] || (defining[i].len() == 3 && defining[i] == defining[j]) {
                    return (0, 1);
                }
            }
        }
        if sets[0].iter().any(|class| sets[1..].iter().all(|set| set.contains(class))) {
            return (0, 1);
        }
        for omitted in 0..4 {
            let mut triple = (0..4).filter(|&index| index != omitted);
            let first = sets[triple.next().unwrap()];
            let second = sets[triple.next().unwrap()];
            let third = sets[triple.next().unwrap()];
            if first.iter().filter(|class| second.contains(class) && third.contains(class)).count() >= 2 {
                return (0, 1);
            }
        }

        for fourth in 0..4 {
            let mut ordered = ids;
            ordered.swap(fourth, 3);
            let set = self.classes_of(ordered[0]);
            if set.len() != 1
                || self.classes_of(ordered[1]) != set
                || self.classes_of(ordered[2]) != set
                || self.classes_of(ordered[3]).contains(&set[0])
            {
                continue;
            }
            let plane = self.class(set[0]);
            let k = plane.drop_axis();
            let projected = projected_orientation(
                k,
                self.real(ordered[0]),
                self.real(ordered[1]),
                self.real(ordered[2]),
            );
            // Cyclic projection gives the normal's k component; moving the fourth point is one swap.
            let parity = if fourth == 3 { 1 } else { -1 };
            return (
                parity * projected * plane.normal[k].sign() * self.side(set[0], ordered[3]),
                2,
            );
        }

        let real = ids.map(|id| self.real(id));
        let delta = ids.map(|id| self.displacement(id));
        if let Some(result) = filter3d(real, delta) {
            (result, 30)
        } else {
            (self.exact_orient3d(ids), 31)
        }
    }

    pub(crate) fn side(&self, class: ClassId, v: u32) -> i32 {
        if self.classes_of(v).contains(&class) {
            return 0;
        }
        let [a, b, c] = self.class(class).seed();
        if self.classes_of(v).is_empty() {
            return sign(orient3d(a, b, c, self.real(v)));
        }
        if let Some(result) = filter3d([a, b, c, self.real(v)], [0.0, 0.0, 0.0, self.displacement(v)]) {
            return result;
        }
        self.exact_side(class, v)
    }

    pub(crate) fn orient2d_along(&self, k: usize, a: u32, b: u32, c: u32) -> i32 {
        let i = (k + 1) % 3;
        let j = (k + 2) % 3;
        if [a, b, c].iter().all(|&id| self.unchanged(id, i) && self.unchanged(id, j)) {
            return projected_orientation(k, self.real(a), self.real(b), self.real(c));
        }
        let a_real = self.real(a);
        let b_real = self.real(b);
        let c_real = self.real(c);
        let acx = a_real.component(i) - c_real.component(i);
        let bcx = b_real.component(i) - c_real.component(i);
        let acy = a_real.component(j) - c_real.component(j);
        let bcy = b_real.component(j) - c_real.component(j);
        let det = acx * bcy - acy * bcx;
        let det_sum = (acx * bcy).abs() + (acy * bcx).abs();
        let errbound = 3.3306690738754716e-16 * det_sum;
        let u = (acx * acx + acy * acy).sqrt();
        let v = (bcx * bcx + bcy * bcy).sqrt();
        let du = self.displacement(a) + self.displacement(c);
        let dv = self.displacement(b) + self.displacement(c);
        let error = du * v + u * dv + du * dv;
        if det.abs() > (errbound + error) * (1.0 + 1e-9) {
            return sign(det);
        }
        self.exact_orient2d(k, a, b, c)
    }

    pub(crate) fn compare(&self, axis: usize, a: u32, b: u32) -> Ordering {
        if self.unchanged(a, axis) && self.unchanged(b, axis) {
            let a = self.real(a).component(axis);
            let b = self.real(b).component(axis);
            return if a < b { Ordering::Less } else if a > b { Ordering::Greater } else { Ordering::Equal };
        }
        self.exact_compare(axis, a, b)
    }

    pub(crate) fn coincide(&self, a: u32, b: u32) -> bool {
        a == b
            || (self.defining_of(a).len() == 3 && self.defining_of(a) == self.defining_of(b))
            || (0..3).all(|axis| self.compare(axis, a, b) == Ordering::Equal)
    }

    #[cfg(test)]
    pub(crate) fn exact_evaluations(&self) -> u64 {
        self.exact_evaluations.load(AtomicOrdering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn orient3d_exact(&self, a: u32, b: u32, c: u32, d: u32) -> i32 {
        self.exact_orient3d([a, b, c, d])
    }

    /// The point is its input position exactly: unplaced, or placed where it already was.
    fn at_input(&self, v: u32) -> bool {
        self.points[v as usize].as_ref().is_none_or(|point| point.at_input)
    }

    fn unchanged(&self, v: u32, axis: usize) -> bool {
        if self.at_input(v) {
            return true;
        }
        match self.defining_of(v) {
            [] => true,
            &[class] => self.class(class).drop_axis() != axis,
            _ => false,
        }
    }

    pub(crate) fn homogeneous(&self, v: u32) -> [Dyadic; 4] {
        match &self.points[v as usize] {
            Some(point) => point.coordinates.clone(),
            None => {
                let [x, y, z] = exact_vector(self.real(v));
                [x, y, z, Dyadic::from_f64(1.0)]
            }
        }
    }

    fn exact_orient3d(&self, ids: [u32; 4]) -> i32 {
        self.exact_evaluations.fetch_add(1, AtomicOrdering::Relaxed);
        let [a, b, c, d] = ids.map(|id| self.homogeneous(id));
        // CSG uses the opposite sign of the homogeneous determinant.
        -determinant4([&a, &b, &c, &d]).sign()
    }

    fn exact_side(&self, class: ClassId, v: u32) -> i32 {
        self.exact_evaluations.fetch_add(1, AtomicOrdering::Relaxed);
        let plane = self.class(class);
        let [x, y, z, weight] = self.homogeneous(v);
        dot(plane.normal.each_ref(), [&x, &y, &z])
            .sub(&plane.offset.mul(&weight))
            .sign()
    }

    fn exact_orient2d(&self, k: usize, a: u32, b: u32, c: u32) -> i32 {
        self.exact_evaluations.fetch_add(1, AtomicOrdering::Relaxed);
        let i = (k + 1) % 3;
        let j = (k + 2) % 3;
        let [a, b, c] = [a, b, c].map(|id| self.homogeneous(id));
        determinant3([&a[i], &a[j], &a[3]], [&b[i], &b[j], &b[3]], [&c[i], &c[j], &c[3]]).sign()
    }

    fn exact_compare(&self, axis: usize, a: u32, b: u32) -> Ordering {
        self.exact_evaluations.fetch_add(1, AtomicOrdering::Relaxed);
        let a = self.homogeneous(a);
        let b = self.homogeneous(b);
        a[axis].mul(&b[3])
            .sub(&b[axis].mul(&a[3]))
            .sign()
            .cmp(&0)
    }
}

fn exact_vector(v: Vec3d) -> [Dyadic; 3] {
    [v.x, v.y, v.z].map(Dyadic::from_f64)
}

fn cross(a: [&Dyadic; 3], b: [&Dyadic; 3]) -> [Dyadic; 3] {
    std::array::from_fn(|axis| {
        let i = (axis + 1) % 3;
        let j = (axis + 2) % 3;
        a[i].mul(b[j]).sub(&a[j].mul(b[i]))
    })
}

fn dot(a: [&Dyadic; 3], b: [&Dyadic; 3]) -> Dyadic {
    a[0].mul(b[0])
        .add(&a[1].mul(b[1]))
        .add(&a[2].mul(b[2]))
}

pub(crate) fn determinant3(a: [&Dyadic; 3], b: [&Dyadic; 3], c: [&Dyadic; 3]) -> Dyadic {
    dot(a, cross(b, c).each_ref())
}

pub(crate) fn determinant4(rows: [&[Dyadic; 4]; 4]) -> Dyadic {
    let [a, b, c, d] = rows;
    let [av, bv, cv, dv] = rows.map(|row| [&row[0], &row[1], &row[2]]);
    let ma = determinant3(bv, cv, dv);
    let mb = determinant3(av, cv, dv);
    let mc = determinant3(av, bv, dv);
    let md = determinant3(av, bv, cv);
    // Expansion along the weight column has signs -, +, -, +.
    a[3].mul(&ma)
        .negate()
        .add(&b[3].mul(&mb))
        .sub(&c[3].mul(&mc))
        .add(&d[3].mul(&md))
}

fn dominant_axis(v: &[Dyadic; 3]) -> usize {
    Vec3d::new(v[0].estimate(), v[1].estimate(), v[2].estimate()).dominant_axis()
}

fn sign(value: f64) -> i32 {
    if value > 0.0 { 1 } else if value < 0.0 { -1 } else { 0 }
}

fn projected_orientation(k: usize, a: Vec3d, b: Vec3d, c: Vec3d) -> i32 {
    let i = (k + 1) % 3;
    let j = (k + 2) % 3;
    sign(orient2d(
        a.component(i), a.component(j),
        b.component(i), b.component(j),
        c.component(i), c.component(j),
    ))
}

pub(crate) fn filter3d(real: [Vec3d; 4], delta: [f64; 4]) -> Option<i32> {
    let [a, b, c, d] = real;
    let adx = a.x - d.x;
    let ady = a.y - d.y;
    let adz = a.z - d.z;
    let bdx = b.x - d.x;
    let bdy = b.y - d.y;
    let bdz = b.z - d.z;
    let cdx = c.x - d.x;
    let cdy = c.y - d.y;
    let cdz = c.z - d.z;

    let bdxcdy = bdx * cdy;
    let cdxbdy = cdx * bdy;
    let cdxady = cdx * ady;
    let adxcdy = adx * cdy;
    let adxbdy = adx * bdy;
    let bdxady = bdx * ady;
    let det = adz * (bdxcdy - cdxbdy) + bdz * (cdxady - adxcdy) + cdz * (adxbdy - bdxady);
    let permanent = (bdxcdy.abs() + cdxbdy.abs()) * adz.abs()
        + (cdxady.abs() + adxcdy.abs()) * bdz.abs()
        + (adxbdy.abs() + bdxady.abs()) * cdz.abs();
    let errbound = 7.7715611723761027e-16 * permanent;

    let u = (a - d).length();
    let v = (b - d).length();
    let w = (c - d).length();
    let du = delta[0] + delta[3];
    let dv = delta[1] + delta[3];
    let dw = delta[2] + delta[3];
    // Hadamard bounds every displacement term separately, so cancellation cannot shrink the bound.
    let error = du * v * w + u * dv * w + u * v * dw
        + du * dv * w + du * v * dw + u * dv * dw + du * dv * dw;
    if det.abs() > (errbound + error) * (1.0 + 1e-9) {
        Some(-sign(det))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_predicates_agree() {
        fn require_sync<T: Sync>() {}
        require_sync::<VirtualPoints>();

        // The exact determinant for [7, 8, 9, 15] needs bits below 2^-1074.
        let seed_a = [
            Vec3d::new(0.1, 0.2, 1.0 + 3e-10),
            Vec3d::new(2.3, 0.1, 1.0 + 1e-10),
            Vec3d::new(0.7, 1.9, 1.0 - 2e-10),
        ];
        let mut state = 7_u64;
        let mut jitter = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((state >> 11) as f64 / (1_u64 << 53) as f64 - 0.5) * 6e-9
        };
        let mut real = vec![
            Vec3d::new(0.2, 0.3, 1.0),
            Vec3d::new(1.6, 0.2, 1.0),
            Vec3d::new(0.4, 1.5, 1.0),
            Vec3d::new(1.4, 1.3, 1.0),
            Vec3d::new(-0.3, 0.9, 1.0),
            Vec3d::new(2.0, -0.2, 0.4),
            Vec3d::new(2.0, 1.4, 1.7),
            Vec3d::new(2.0, 0.4, 1.0),
            Vec3d::new(2.0, 1.1, 1.0),
            Vec3d::new(2.0, 0.0, 1.0),
            Vec3d::new(2.0, 0.0, 1.0),
            seed_a[0] + Vec3d::new(0.0, 0.0, 1.2e-8),
            Vec3d::new(-0.7, 0.5, 2.1),
            Vec3d::new(3.0, 1.8, -0.7),
            Vec3d::new(0.6, -1.4, 3.2),
            Vec3d::new(1.0, 0.8, 1.0),
            seed_a[0],
        ];
        for id in (0..10).chain(std::iter::once(15)) {
            real[id] += Vec3d::new(jitter(), jitter(), jitter());
        }
        real[10] = real[9] + Vec3d::new(3e-8, 0.0, 0.0);
        let mut points = VirtualPoints::new(real.clone());
        let a = points.add_class(seed_a).expect("class A must have a normal");
        let b = points.add_class([
            Vec3d::new(2.0 + 2e-10, -0.3, 0.0),
            Vec3d::new(2.0, 2.4, 0.1),
            Vec3d::new(2.0 - 3e-10, 0.2, 2.5),
        ]).expect("class B must have a normal");
        let c = points.add_class([
            Vec3d::new(-0.2, 1e-10, 0.1),
            Vec3d::new(2.2, -2e-10, 0.3),
            Vec3d::new(0.4, 0.0, 2.1),
        ]).expect("class C must have a normal");
        let d = points.add_class([
            Vec3d::new(1.0, 0.0, 1.0),
            Vec3d::new(2.0, 0.0, 1.05),
            Vec3d::new(1.0, 2.0, 1.0),
        ]).expect("class D must have a normal");
        let e = points.add_class([
            Vec3d::new(0.0, 0.0, 3.0),
            Vec3d::new(1.0, 0.0, 3.0),
            Vec3d::new(0.0, 1.0, 3.0),
        ]).expect("class E must have a normal");
        let f = points.add_class([
            Vec3d::new(0.0, 0.0, 5.0),
            Vec3d::new(1.0, 0.0, 5.0),
            Vec3d::new(0.0, 1.0, 5.0),
        ]).expect("class F must have a normal");
        assert_eq!(
            points.add_class([Vec3d::ZERO, Vec3d::X, Vec3d::X * 2.0]),
            None,
            "collinear seed ids 0, 1, 2 must not define a class"
        );
        assert!(points.place(0, &[]).is_none(), "empty class set at point 0");
        assert!(points.place(0, &[a, b, c, d]).is_none(), "four classes at point 0");
        assert!(points.place(0, &[e, f]).is_none(), "parallel classes {e}, {f} at point 0");
        assert!(points.place(0, &[a, e, f]).is_none(), "dependent classes {a}, {e}, {f} at point 0");

        let assignments = [
            vec![a], vec![a], vec![a], vec![a], vec![a],
            vec![b], vec![b], vec![b, a], vec![a, b],
            vec![c, a, b], vec![b, c, a],
            vec![], vec![], vec![], vec![], vec![d, a], vec![],
        ];
        for (id, classes) in assignments.iter().enumerate() {
            if !classes.is_empty() {
                let placed = points.place(id as u32, classes)
                    .unwrap_or_else(|| panic!("placement for point {id}, classes {classes:?}"));
                assert_eq!(placed.coordinates[3].sign(), 1, "positive weight for point {id}");
                points.set(id as u32, Some(placed));
            }
            let mut sorted = classes.clone();
            sorted.sort_unstable();
            assert_eq!(points.classes_of(id as u32), sorted.as_slice(), "sorted class set for point {id}");
            assert_eq!(points.real(id as u32), real[id], "input position preserved for point {id}");
        }
        assert_eq!(points.exact_evaluations(), 0, "placement of points 0..16 is not a predicate");

        let count = real.len() as u32;
        let mut tiers = [0_u32; 32];
        let mut quadruples = 0;
        for i in 0..count {
            for j in i + 1..count {
                for k in j + 1..count {
                    for l in k + 1..count {
                        let ids = [i, j, k, l];
                        let (result, tier) = points.orient3d_tier(i, j, k, l);
                        tiers[tier as usize] += 1;
                        assert_eq!(result, points.orient3d_exact(i, j, k, l), "exact orient3d for {ids:?}, tier {tier}");
                        assert_eq!(points.orient3d(i, j, k, l), result, "public orient3d for {ids:?}");
                        assert_eq!(points.orient3d(j, i, k, l), -result, "first swap for {ids:?}");
                        assert_eq!(points.orient3d(i, j, l, k), -result, "last swap for {ids:?}");
                        if quadruples < 20 {
                            for p in 0..4 {
                                for q in 0..4 {
                                    for r in 0..4 {
                                        if p == q || p == r || q == r {
                                            continue;
                                        }
                                        let s = 6 - p - q - r;
                                        let order = [p, q, r, s];
                                        let mut parity = 1;
                                        for left in 0..4 {
                                            for right in left + 1..4 {
                                                if order[left] > order[right] {
                                                    parity = -parity;
                                                }
                                            }
                                        }
                                        let permuted = order.map(|index| ids[index]);
                                        assert_eq!(
                                            points.orient3d(permuted[0], permuted[1], permuted[2], permuted[3]),
                                            result * parity,
                                            "permutation {permuted:?} of {ids:?}"
                                        );
                                    }
                                }
                            }
                        }
                        quadruples += 1;
                    }
                }
            }
        }
        for tier in [0, 1, 2, 30, 31] {
            assert!(tiers[tier] > 0, "tier {tier} must occur among point ids 0..{count}");
        }
        // The unclassed seed and a mixed A/fold triple are coplanar without a class shortcut.
        assert_eq!(points.orient3d_tier(0, 1, 7, 16), (0, 31), "exact fallback for ids [0, 1, 7, 16]");
        assert_eq!(points.orient3d_tier(0, 0, 5, 6), (0, 1), "repeated id in [0, 0, 5, 6]");
        assert_eq!(points.orient3d_tier(7, 8, 9, 12), (0, 1), "shared line for ids [7, 8, 9, 12]");

        for class in [a, b, c, d, e, f] {
            for id in 0..count {
                let side = points.side(class, id);
                assert_eq!(side, points.exact_side(class, id), "exact side for class {class}, point {id}");
                if points.classes_of(id).contains(&class) {
                    assert_eq!(side, 0, "membership side for class {class}, point {id}");
                }
            }
        }
        assert_eq!(points.side(a, 11), 1, "nearby unclassed point 11 remains above class {a}");

        let members: Vec<_> = (0..count).filter(|&id| points.classes_of(id).contains(&a)).collect();
        for (index, &i) in members.iter().enumerate() {
            for (next, &j) in members.iter().enumerate().skip(index + 1) {
                for &k in members.iter().skip(next + 1) {
                    let result = points.orient2d_along(points.class(a).drop_axis(), i, j, k);
                    assert_eq!(result, points.exact_orient2d(points.class(a).drop_axis(), i, j, k), "exact orient2d class {a}, ids [{i}, {j}, {k}]");
                    assert_eq!(points.orient2d_along(points.class(a).drop_axis(), j, i, k), -result, "orient2d first swap class {a}, ids [{i}, {j}, {k}]");
                    assert_eq!(points.orient2d_along(points.class(a).drop_axis(), i, k, j), -result, "orient2d last swap class {a}, ids [{i}, {j}, {k}]");
                }
            }
        }
        for i in 0..count {
            for j in i..count {
                for axis in 0..3 {
                    let result = points.compare(axis, i, j);
                    assert_eq!(result, points.exact_compare(axis, i, j), "exact compare axis {axis}, ids [{i}, {j}]");
                    assert_eq!(points.compare(axis, j, i), result.reverse(), "reverse compare axis {axis}, ids [{i}, {j}]");
                }
            }
            let distance = (points.approx(i) - points.real(i)).length();
            let delta = points.displacement(i);
            let slack = 1e-15 * (1.0 + points.real(i).length());
            if points.classes_of(i).is_empty() {
                assert_eq!(delta, 0.0, "zero displacement for unclassed point {i}");
                assert_eq!(points.approx(i), points.real(i), "unchanged unclassed point {i}");
            } else {
                assert!(distance <= delta + slack, "displacement bound for point {i}: {distance} <= {delta} + {slack}");
                assert!(delta <= 1.01 * distance + slack, "tight displacement bound for point {i}: {delta}, {distance}");
            }
        }
        assert!(points.coincide(9, 10), "corner ids [9, 10] coincide");
        assert!(!points.coincide(7, 8), "fold ids [7, 8] differ");
        let before = points.exact_evaluations();
        points.orient3d_exact(0, 1, 7, 16);
        assert_eq!(points.exact_evaluations(), before + 1, "one exact evaluation for ids [0, 1, 7, 16]");
        points.set(0, None);
        assert!(points.classes_of(0).is_empty(), "clear classes for point 0");
        assert_eq!(points.approx(0), points.real(0), "clear virtual position for point 0");
        assert_eq!(points.displacement(0), 0.0, "clear displacement for point 0");
    }

    #[test]
    fn cyclic_projection_signs() {
        for k in 0..3 {
            let position = |x, y, z| {
                let mut coordinates = [0.0; 3];
                coordinates[(k + 1) % 3] = x;
                coordinates[(k + 2) % 3] = y;
                coordinates[k] = z;
                Vec3d::new(coordinates[0], coordinates[1], coordinates[2])
            };
            for reversed in [false, true] {
                let mut seed = [position(0.0, 0.0, 2.0), position(1.0, 0.0, 2.0), position(0.0, 1.0, 2.0)];
                if reversed {
                    seed.swap(1, 2);
                }
                let mut points = VirtualPoints::new(vec![
                    position(0.0, 0.0, 2.0 + 1e-9),
                    position(1.0, 0.0, 2.0 - 2e-9),
                    position(0.0, 1.0, 2.0 + 3e-9),
                    position(0.3, 0.4, 3.0),
                ]);
                let class = points.add_class(seed).expect("axis plane must have a normal");
                assert_eq!(points.class(class).drop_axis(), k, "dropped axis for class {class}, axis {k}");
                for id in 0..3 {
                    let placed = points.place(id, &[class])
                        .unwrap_or_else(|| panic!("axis {k}, reversed {reversed}, point {id} placement"));
                    assert_eq!(placed.coordinates[3].sign(), 1, "positive weight for axis {k}, reversed {reversed}, point {id}");
                    points.set(id, Some(placed));
                }
                assert_eq!(points.orient2d_along(points.class(class).drop_axis(), 0, 1, 2), 1, "cyclic projection for axis {k}, reversed {reversed}, ids [0, 1, 2]");
                assert_eq!(points.exact_orient2d(points.class(class).drop_axis(), 0, 1, 2), 1, "exact projection for axis {k}, reversed {reversed}, ids [0, 1, 2]");
                assert_eq!(points.side(class, 3), if reversed { -1 } else { 1 }, "seed winding for axis {k}, point 3");
                assert_eq!(points.orient3d_tier(0, 1, 2, 3), (1, 2), "factor sign for axis {k}, reversed {reversed}, ids [0, 1, 2, 3]");
                assert_eq!(points.orient3d_exact(0, 1, 2, 3), 1, "exact sign for axis {k}, reversed {reversed}, ids [0, 1, 2, 3]");
                for fourth in 0..3 {
                    let mut ids = [0, 1, 2, 3];
                    ids.swap(fourth, 3);
                    assert_eq!(
                        points.orient3d_tier(ids[0], ids[1], ids[2], ids[3]),
                        (-1, 2),
                        "factor swap for axis {k}, reversed {reversed}, ids {ids:?}"
                    );
                }
            }
        }
    }
}
