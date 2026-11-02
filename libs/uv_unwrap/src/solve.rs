//! Sparse Hermitian solver: conjugate gradients preconditioned by a W-cycle
//! over pairwise vertex aggregates.
//!
//! A chart's conformal system is a cotangent Laplacian with a boundary-only
//! imaginary part, pinned at just two vertices. Its condition number grows
//! with the chart, so plain or diagonally scaled CG takes thousands of steps
//! on a dense chart. Aggregation keeps the step count nearly flat instead.

use crate::Checkpoint;

pub(crate) type C = [f64; 2];

#[inline]
fn cmul(a: C, b: C) -> C {
    [a[0] * b[0] - a[1] * b[1], a[0] * b[1] + a[1] * b[0]]
}

/// A Hermitian matrix with both triangles stored; `diag` is its real diagonal.
pub(crate) struct Csr {
    row: Vec<u32>,
    col: Vec<u32>,
    val: Vec<C>,
    diag: Vec<f64>,
}

impl Csr {
    /// Sums duplicate entries. `triplets` holds both triangles.
    pub(crate) fn from_triplets(n: usize, triplets: &[(u32, u32, C)]) -> Self {
        let mut count = vec![0u32; n + 1];
        for t in triplets {
            count[t.0 as usize + 1] += 1;
        }
        for i in 0..n {
            count[i + 1] += count[i];
        }
        let mut order = vec![0u32; triplets.len()];
        let mut next = count.clone();
        for (at, t) in triplets.iter().enumerate() {
            let slot = &mut next[t.0 as usize];
            order[*slot as usize] = at as u32;
            *slot += 1;
        }
        let mut row = Vec::with_capacity(n + 1);
        row.push(0u32);
        let mut col = Vec::with_capacity(triplets.len() / 2);
        let mut val: Vec<C> = Vec::with_capacity(triplets.len() / 2);
        let mut seen = vec![u32::MAX; n];
        for i in 0..n {
            let start = col.len();
            for &at in &order[count[i] as usize..count[i + 1] as usize] {
                let t = &triplets[at as usize];
                let j = t.1 as usize;
                if seen[j] != u32::MAX && seen[j] as usize >= start {
                    let v = &mut val[seen[j] as usize];
                    v[0] += t.2[0];
                    v[1] += t.2[1];
                } else {
                    seen[j] = col.len() as u32;
                    col.push(t.1);
                    val.push(t.2);
                }
            }
            row.push(col.len() as u32);
        }
        Self::finish(row, col, val)
    }

    fn finish(row: Vec<u32>, col: Vec<u32>, val: Vec<C>) -> Self {
        let n = row.len() - 1;
        let mut diag = vec![0.0; n];
        for i in 0..n {
            for at in row[i] as usize..row[i + 1] as usize {
                if col[at] as usize == i {
                    diag[i] = val[at][0];
                }
            }
        }
        // A vertex every triangle of which collapsed has no equation at all.
        // A sliver of regularisation keeps the system definite; the vertex
        // then stays where the warm start put it.
        let mean = diag.iter().sum::<f64>() / n.max(1) as f64;
        let floor = (mean * 1.0e-12).max(f64::MIN_POSITIVE);
        for d in &mut diag {
            *d = d.max(floor);
        }
        Self { row, col, val, diag }
    }

    pub(crate) fn n(&self) -> usize {
        self.diag.len()
    }

    pub(crate) fn nnz(&self) -> usize {
        self.col.len()
    }

    fn mul(&self, x: &[C], y: &mut [C]) {
        for i in 0..self.n() {
            let mut s = [0.0; 2];
            for at in self.row[i] as usize..self.row[i + 1] as usize {
                let p = cmul(self.val[at], x[self.col[at] as usize]);
                s[0] += p[0];
                s[1] += p[1];
            }
            y[i] = s;
        }
    }

    #[inline]
    fn relax(&self, i: usize, b: &[C], x: &mut [C]) {
        let mut s = b[i];
        for at in self.row[i] as usize..self.row[i + 1] as usize {
            let j = self.col[at] as usize;
            if j != i {
                let p = cmul(self.val[at], x[j]);
                s[0] -= p[0];
                s[1] -= p[1];
            }
        }
        x[i] = [s[0] / self.diag[i], s[1] / self.diag[i]];
    }

    fn sweep_forward(&self, b: &[C], x: &mut [C]) {
        for i in 0..self.n() {
            self.relax(i, b, x);
        }
    }

    fn sweep_backward(&self, b: &[C], x: &mut [C]) {
        for i in (0..self.n()).rev() {
            self.relax(i, b, x);
        }
    }

    /// One round of strongest-neighbour matching. Returns each node's aggregate
    /// and the aggregate count.
    fn match_pairs(&self) -> (Vec<u32>, usize) {
        let n = self.n();
        let mut aggregate = vec![u32::MAX; n];
        let mut count = 0u32;
        for i in 0..n {
            if aggregate[i] != u32::MAX {
                continue;
            }
            let mut largest = 0.0f64;
            let mut best = (0.0f64, usize::MAX);
            for at in self.row[i] as usize..self.row[i + 1] as usize {
                let j = self.col[at] as usize;
                if j == i {
                    continue;
                }
                let v = self.val[at];
                let strength = v[0] * v[0] + v[1] * v[1];
                largest = largest.max(strength);
                if aggregate[j] == u32::MAX && strength > best.0 {
                    best = (strength, j);
                }
            }
            aggregate[i] = count;
            if best.1 != usize::MAX && best.0 >= largest * 0.0625 {
                aggregate[best.1] = count;
            }
            count += 1;
        }
        (aggregate, count as usize)
    }

    /// `P^H A P` for the piecewise-constant prolongation of `aggregate`.
    fn galerkin(&self, aggregate: &[u32], coarse: usize) -> Csr {
        let mut start = vec![0u32; coarse + 1];
        for &a in aggregate {
            start[a as usize + 1] += 1;
        }
        for i in 0..coarse {
            start[i + 1] += start[i];
        }
        let mut next = start.clone();
        let mut members = vec![0u32; aggregate.len()];
        for (i, &a) in aggregate.iter().enumerate() {
            members[next[a as usize] as usize] = i as u32;
            next[a as usize] += 1;
        }
        let mut row = Vec::with_capacity(coarse + 1);
        row.push(0u32);
        let mut col = Vec::with_capacity(self.nnz() / 2);
        let mut val: Vec<C> = Vec::with_capacity(self.nnz() / 2);
        let mut seen = vec![u32::MAX; coarse];
        for c in 0..coarse {
            let first = col.len();
            for &i in &members[start[c] as usize..start[c + 1] as usize] {
                let i = i as usize;
                for at in self.row[i] as usize..self.row[i + 1] as usize {
                    let target = aggregate[self.col[at] as usize];
                    let slot = seen[target as usize];
                    if slot != u32::MAX && slot as usize >= first {
                        let v = &mut val[slot as usize];
                        v[0] += self.val[at][0];
                        v[1] += self.val[at][1];
                    } else {
                        seen[target as usize] = col.len() as u32;
                        col.push(target);
                        val.push(self.val[at]);
                    }
                }
            }
            row.push(col.len() as u32);
        }
        Csr::finish(row, col, val)
    }
}

/// Size at which the coarsest system is factored outright.
const DENSE_MAX: usize = 48;

/// Dense `L L^H` of a small Hermitian system.
struct Dense {
    n: usize,
    l: Vec<C>,
}

impl Dense {
    fn new(a: &Csr) -> Self {
        let n = a.n();
        let mut l = vec![[0.0; 2]; n * n];
        for i in 0..n {
            for at in a.row[i] as usize..a.row[i + 1] as usize {
                l[i * n + a.col[at] as usize] = a.val[at];
            }
            l[i * n + i] = [a.diag[i], 0.0];
        }
        let scale = a.diag.iter().fold(0.0f64, |m, d| m.max(*d));
        let floor = (scale * 1.0e-13).max(f64::MIN_POSITIVE);
        for j in 0..n {
            let mut d = l[j * n + j][0];
            for k in 0..j {
                let v = l[j * n + k];
                d -= v[0] * v[0] + v[1] * v[1];
            }
            let d = d.max(floor).sqrt();
            l[j * n + j] = [d, 0.0];
            for i in j + 1..n {
                let mut s = l[i * n + j];
                for k in 0..j {
                    let a = l[i * n + k];
                    let b = l[j * n + k];
                    // a * conj(b)
                    s[0] -= a[0] * b[0] + a[1] * b[1];
                    s[1] -= a[1] * b[0] - a[0] * b[1];
                }
                l[i * n + j] = [s[0] / d, s[1] / d];
            }
        }
        Self { n, l }
    }

    fn solve(&self, b: &[C], x: &mut [C]) {
        let n = self.n;
        for i in 0..n {
            let mut s = b[i];
            for k in 0..i {
                let p = cmul(self.l[i * n + k], x[k]);
                s[0] -= p[0];
                s[1] -= p[1];
            }
            let d = self.l[i * n + i][0];
            x[i] = [s[0] / d, s[1] / d];
        }
        for i in (0..n).rev() {
            let mut s = x[i];
            for k in i + 1..n {
                let a = self.l[k * n + i];
                let v = x[k];
                // conj(a) * v
                s[0] -= a[0] * v[0] + a[1] * v[1];
                s[1] -= a[0] * v[1] - a[1] * v[0];
            }
            let d = self.l[i * n + i][0];
            x[i] = [s[0] / d, s[1] / d];
        }
    }
}

struct Level {
    a: Csr,
    /// Fine node to coarse node of the next level; empty on the last.
    aggregate: Vec<u32>,
    residual: Vec<C>,
    rhs: Vec<C>,
    x: Vec<C>,
}

impl Level {
    fn new(a: Csr, aggregate: Vec<u32>) -> Self {
        let n = a.n();
        Self {
            a,
            aggregate,
            residual: vec![[0.0; 2]; n],
            rhs: vec![[0.0; 2]; n],
            x: vec![[0.0; 2]; n],
        }
    }
}

struct Hierarchy {
    levels: Vec<Level>,
    dense: Option<Dense>,
}

impl Hierarchy {
    fn new(a: Csr) -> Self {
        let mut levels = Vec::new();
        let mut current = a;
        while current.n() > DENSE_MAX {
            let (first, first_count) = current.match_pairs();
            let middle = current.galerkin(&first, first_count);
            let (second, count) = middle.match_pairs();
            if count * 5 > current.n() * 4 {
                break;
            }
            let coarse = middle.galerkin(&second, count);
            let aggregate = first.iter().map(|&a| second[a as usize]).collect();
            levels.push(Level::new(current, aggregate));
            current = coarse;
        }
        let dense = (current.n() <= DENSE_MAX).then(|| Dense::new(&current));
        levels.push(Level::new(current, Vec::new()));
        Self { levels, dense }
    }

    /// Applies the preconditioner to `levels[0].rhs`, into `levels[0].x`.
    fn cycle(levels: &mut [Level], dense: Option<&Dense>) {
        let (level, rest) = levels.split_first_mut().unwrap();
        let n = level.a.n();
        level.x[..n].fill([0.0; 2]);
        if rest.is_empty() {
            match dense {
                Some(d) => d.solve(&level.rhs, &mut level.x),
                None => {
                    for _ in 0..4 {
                        level.a.sweep_forward(&level.rhs, &mut level.x);
                        level.a.sweep_backward(&level.rhs, &mut level.x);
                    }
                }
            }
            return;
        }
        level.a.sweep_forward(&level.rhs, &mut level.x);
        // Twice below this level: with aggregates of about four nodes the
        // W-cycle still costs a bounded multiple of the fine sweep, and the
        // piecewise-constant transfer needs the second correction to stay
        // independent of the chart's size.
        for round in 0..2 {
            level.a.mul(&level.x, &mut level.residual);
            let next = &mut rest[0];
            next.rhs.fill([0.0; 2]);
            for i in 0..n {
                let r = [
                    level.rhs[i][0] - level.residual[i][0],
                    level.rhs[i][1] - level.residual[i][1],
                ];
                let slot = &mut next.rhs[level.aggregate[i] as usize];
                slot[0] += r[0];
                slot[1] += r[1];
            }
            Self::cycle(rest, dense);
            let next = &rest[0];
            for i in 0..n {
                let e = next.x[level.aggregate[i] as usize];
                level.x[i][0] += e[0];
                level.x[i][1] += e[1];
            }
            if rest.len() == 1 && round == 0 {
                // The coarsest solve is exact.
                break;
            }
        }
        level.a.sweep_backward(&level.rhs, &mut level.x);
    }
}

/// Solves `a x = b`, from the warm start already in `x`. An iteration cap only
/// stops refining: the caller measures the result and splits a chart that is
/// still too distorted, so nothing fails here. Returns false when cancelled.
pub(crate) fn solve(a: Csr, b: &[C], x: &mut [C], checkpoint: &mut Checkpoint<'_>) -> bool {
    const TOLERANCE: f64 = 1.0e-7;
    const ITERATIONS: usize = 200;
    let n = a.n();
    let nnz = a.nnz() as u64;
    let mut hierarchy = Hierarchy::new(a);
    let dot = |p: &[C], q: &[C]| p.iter().zip(q).map(|(p, q)| p[0] * q[0] + p[1] * q[1]).sum::<f64>();
    let mut r = vec![[0.0; 2]; n];
    hierarchy.levels[0].a.mul(x, &mut r);
    for i in 0..n {
        r[i] = [b[i][0] - r[i][0], b[i][1] - r[i][1]];
    }
    let goal = TOLERANCE * dot(b, b).sqrt().max(f64::MIN_POSITIVE);
    if dot(&r, &r).sqrt() <= goal {
        return true;
    }
    let mut p = vec![[0.0; 2]; n];
    let mut ap = vec![[0.0; 2]; n];
    let mut rz = 0.0;
    for iteration in 0..ITERATIONS {
        if !checkpoint(nnz.saturating_mul(8)) {
            return false;
        }
        hierarchy.levels[0].rhs.copy_from_slice(&r);
        let dense = hierarchy.dense.as_ref();
        Hierarchy::cycle(&mut hierarchy.levels, dense);
        let z = &hierarchy.levels[0].x;
        let rz_next = dot(&r, z);
        let beta = if iteration == 0 { 0.0 } else { rz_next / rz };
        rz = rz_next;
        for i in 0..n {
            p[i] = [z[i][0] + beta * p[i][0], z[i][1] + beta * p[i][1]];
        }
        hierarchy.levels[0].a.mul(&p, &mut ap);
        let curvature = dot(&p, &ap);
        if !(curvature > 0.0) {
            break;
        }
        let alpha = rz / curvature;
        for i in 0..n {
            x[i][0] += alpha * p[i][0];
            x[i][1] += alpha * p[i][1];
            r[i][0] -= alpha * ap[i][0];
            r[i][1] -= alpha * ap[i][1];
        }
        if dot(&r, &r).sqrt() <= goal {
            break;
        }
    }
    true
}
