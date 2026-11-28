//! One drag move takes trust-region Newton steps on the constraint surface. The reduced
//! model tests each weak direction for a ridge or a remaining decrease. The press frame
//! fixes the orientation of the side sheet (Q9, Q20). After a sheet refusal the solver
//! tries a mirror, a projected twin or a cut along the trial ray. At a fold the sheet
//! band admits displacements within projection noise or geometric tolerance (Q17, Q18b).
//!
//! | Quantity | Value and use | Source |
//! | --- | --- | --- |
//! | Rank cut | `1e-9` times the first QR pivot, also used for the strong pin directions | Q7, Q19 amendment |
//! | SHRINK | 1/4 of the refused step length for the next radius | Q7, Q17, Q20 |
//! | c₀ | `1e-8 (‖BᵀB‖ + ‖K‖)`, the curvature floor for the eigenvalue split | Q7, Q19 amendment |
//! | JACOBI_MAX_K | 16: the reduced Hessian's eigen-solver is Jacobi up to this k and QL above it; the grams use Jacobi at every k. Under any exact solver a pose's weak directions resolve to about k ε ‖H‖ / λ⁺ of the step, not to tol (λ⁺ the smallest curvature the step divides by) | R7, Q28 |
//! | Weak set | measured on open chains to 100 links: every round with two or more directions under c₀ floors all of them and zeroes all their slopes, so the step takes nothing from the weak set in any basis | Q28, M3b |
//! | n_eff | `2e-8 ‖r⊥‖ ‖c‖ + 16 ε ‖BᵀB‖`, the weak-direction curvature noise | Q19 amendment |
//! | ν | `‖JZ‖_F ‖r‖ / abs(last pivot)`, zero without a pivot, the projected slope noise | Q7 |
//! | τ | tol/2, the reached distance | Q19 amendment, R8 |
//! | β | `τ max(‖r‖, τ) / 2`, the decrease that settles a weak direction | Q19 amendment |
//! | h | `ε^(1/3) scale`, the central difference step for curvature | Q7 |
//! | res | `(4 ε ‖p‖∞ + ‖Δp_proj‖) ‖r‖`, the resolution of the objective decrease | Q22, Q23, R1, R9 |
//! | Acceptance ratio | 1e-4, the minimum measured fraction of predicted decrease | Q7, Q23 |
//! | Radius growth | ×2 on a boundary acceptance with ratio above 0.75 | Q7 |
//! | Rule (b) gates | toggle: −ev <= 1/32 and linear share <= 1/8; offset: −1/32 <= a_t < 0 and both residual terms <= 1/8 | Q22, Q23b |
//! | MIRROR_NEG | an identity mirror with sᵀHs < 0 and odd share <= 1/8 tries −s | Q21, Q22c part 3 |
//! | RUNAWAY | 1e12 times the sketch scale, the rollback threshold for unmeasured growth | R2 |
//! | TRIAL_CAP | 200 trial iterations per round (the frame's `trial_cap`), then the move keeps its last accepted pose and the next move goes on from it | Probe trial cap |

use super::{dot, max_abs, DragOutcome, DragQr, Jac, Options, PointId, System, RANK_CUT};
#[cfg(test)]
use super::work;
use crate::region::P2;

const SHRINK: f64 = 0.25;
const CURVATURE_FLOOR: f64 = 1e-8;
const RUNAWAY: f64 = 1e12;
const TRIAL_CAP: usize = 200;

impl DragQr {
    fn multipliers(&self, mut pull: Vec<f64>) -> Vec<f64> {
        for (k, &tau) in self.tau.iter().enumerate() {
            Self::reflect(&self.columns[k], tau, k, &mut pull);
        }
        // R lambda_perm = Y^T P^T d. Undo the pivot permutation for J^T lambda.
        let mut lambda = vec![0.0; self.order.len()];
        for k in (0..self.tau.len()).rev() {
            let l = pull[k] / self.columns[k][k];
            lambda[self.order[k]] = l;
            for (rhs, &r) in pull[..k].iter_mut().zip(&self.columns[k]) {
                *rhs -= r * l;
            }
        }
        lambda
    }

    /// Minimum-norm correction for the independent constraint rows.
    fn chord(&self, residuals: &[f64], n: usize) -> Vec<f64> {
        let mut delta = vec![0.0; n];
        for k in 0..self.tau.len() {
            let col = &self.columns[k];
            delta[k] = (-residuals[self.order[k]] - dot(&col[..k], &delta[..k])) / col[k];
        }
        for k in (0..self.tau.len()).rev() {
            Self::reflect(&self.columns[k], self.tau[k], k, &mut delta);
        }
        delta
    }

    fn noise_floor(&self, jac: &Jac, distance: f64) -> f64 {
        let Some(last) = self.tau.len().checked_sub(1) else { return 0.0 };
        let mut squared = 0.0;
        for v in &self.z {
            for i in 0..jac.rows() {
                let projection = jac.row(i).iter().map(|&(a, value)| value * v[a]).sum::<f64>();
                squared += projection.powi(2);
            }
        }
        squared.sqrt() / self.columns[last][last].abs() * distance
    }

    fn point_mobility(&self, point: usize) -> usize {
        let mut g = [[0.0; 2]; 2];
        for v in &self.z {
            for i in 0..2 {
                for j in 0..2 {
                    g[i][j] += v[point + i] * v[point + j];
                }
            }
        }
        let trace = g[0][0] + g[1][1];
        let gap = (g[0][0] - g[1][1]).hypot(2.0 * g[0][1]);
        [0.5 * (trace + gap), 0.5 * (trace - gap)].iter().filter(|&&ev| ev > 1e-14).count()
    }
}

fn norm(v: &[f64]) -> f64 {
    dot(v, v).sqrt()
}

fn sign(x: f64) -> i32 {
    if x > 0.0 { 1 } else if x < 0.0 { -1 } else { 0 }
}

fn determinant(mut a: Vec<Vec<f64>>) -> f64 {
    #[cfg(test)]
    super::work::add(|w| w.determinant += (a.len() as u64).pow(3));
    let mut det = 1.0;
    for j in 0..a.len() {
        let p = (j..a.len()).max_by(|&i, &k| a[i][j].abs().total_cmp(&a[k][j].abs())).unwrap();
        if a[p][j] == 0.0 { return 0.0; }
        if j != p { a.swap(j, p); det = -det; }
        det *= a[j][j];
        for i in j + 1..a.len() {
            let f = a[i][j] / a[j][j];
            for l in j + 1..a.len() { a[i][l] -= f * a[j][l]; }
        }
    }
    det
}

#[derive(Clone, Copy, Debug, Default)]
struct SheetRead {
    reach: usize,
    omega: i32,
    det: f64,
    sigma: i32,
}

#[derive(Clone, Copy)]
struct SheetChord {
    before: f64,
    after: f64,
    length: f64,
    alpha: f64,
    past: Option<f64>,
}

impl SheetChord {
    fn new(frame: &PressFrame, next: SheetRead, pose: &[f64], trial: &[f64]) -> Self {
        let oriented = |s: SheetRead| f64::from(frame.press_sign * s.omega) * s.det;
        let before = oriented(frame.current);
        let after = oriented(next);
        let length = pose.iter().zip(trial).map(|(a,b)| (a-b).powi(2)).sum::<f64>().sqrt();
        let denominator = before.max(0.0) + after.abs();
        let alpha = if denominator > 0.0 { before.max(0.0) / denominator } else { 0.0 };
        let past = if before > 0.0 && after <= 0.0 {
            Some(length * after.abs() / denominator)
        } else { None };
        Self { before, after, length, alpha, past }
    }
}

#[derive(Clone, Debug)]
pub(super) struct PressFrame {
    anchor: Vec<f64>,
    pub(super) branches: Vec<f64>,
    point: usize,
    nullity: usize,
    /// The spare directions are independent of the fold (Q21, Q22c addendum): σ_press then applies.
    /// Judged at the two axis pulls in the frame's first round, then frozen. A rebuilt frame judges it again.
    independent_spare: bool,
    /// Whether `independent_spare` has been computed for this frame.
    spare_judged: bool,
    reach: usize,
    spare: Vec<Vec<f64>>,
    tau: P2,
    tau_point: P2,
    pivot_rows: Vec<usize>,
    press_sign: i32,
    /// k = r at the press: no spare direction. sigma_press is then written once, when the frame is built.
    k_eq_r: bool,
    current: SheetRead,
    /// Trials per round before the move keeps its last accepted pose: TRIAL_CAP.
    pub(super) trial_cap: usize,
}

impl PressFrame {
    pub(super) fn new(s: &System, p: PointId, scale: f64) -> Self {
        let point = s.points[p.0];
        let branches = s.branches();
        let (_, jac, _) = s.evaluate(&s.x, scale, &branches);
        let qr = DragQr::new(&jac, s.x.len());
        let mut frame = Self { anchor: s.x.clone(), branches, point, nullity: 0, independent_spare: false, spare_judged: false, reach: 0, spare: Vec::new(),
            tau: [0.0; 2], tau_point: s.at(p), pivot_rows: Vec::new(), press_sign: 0, k_eq_r: false, current: SheetRead::default(), trial_cap: TRIAL_CAP };
        frame.set_basis(&qr);
        frame.k_eq_r = frame.nullity == frame.reach;
        frame
    }

    /// σ_press is enforced: k = r at the press, or an independent spare (Q9 verdict 4,
    /// Q21). False in the coupled-spare class, where no sheet separates the sides.
    /// Before the frame's first round, the spare class is not known yet, and a non-empty spare reads false.
    pub(super) fn side_enforced(&self) -> bool {
        self.spare.is_empty() || self.independent_spare
    }

    /// Q21 keying: k = r, or k > r with an independent spare. sigma_press is immutable there.
    fn immutable(&self) -> bool {
        self.k_eq_r || self.independent_spare
    }

    fn sheet_dim(&self) -> usize {
        self.nullity - self.spare.len()
    }

    fn set_basis(&mut self, qr: &DragQr) {
        let point = self.point;
        let reach = qr.point_mobility(point);
        let b: Vec<&[f64]> = qr.z.iter().map(|z| &z[point..point + 2]).collect();
        let gram = b.iter().map(|a| b.iter().map(|v| dot(a, v)).collect()).collect();
        let eig = SmallEigen::jacobi(gram);
        let spare = eig.vectors.iter().take(qr.z.len() - reach).map(|u|
            (0..self.anchor.len()).map(|i| qr.z.iter().zip(u).map(|(z, v)| z[i] * v).sum()).collect()
        ).collect();
        let tau = if reach == 1 { Self::tangent(&qr, point) } else { [0.0; 2] };
        let mut pivot_rows = qr.order[..qr.tau.len()].to_vec();
        pivot_rows.sort_unstable();
        self.nullity = qr.z.len();
        self.reach = reach;
        self.spare = spare;
        self.tau = tau;
        self.pivot_rows = pivot_rows;
        self.current = self.read(qr, tau);
        self.press_sign = self.current.sigma;
    }

    fn tangent(qr: &DragQr, point: usize) -> P2 {
        let gram = (0..2).map(|i| (0..2).map(|j|
            qr.z.iter().map(|z| z[point + i] * z[point + j]).sum()).collect()).collect();
        let eig = SmallEigen::jacobi(gram);
        [eig.vectors[1][0], eig.vectors[1][1]]
    }

    fn trial_tangent(&self, qr: &DragQr, at: P2, tol: f64) -> P2 {
        if self.reach != 1 || qr.point_mobility(self.point) != 1 { return self.tau; }
        let b = Self::tangent(qr, self.point);
        let c = [at[0] - self.tau_point[0], at[1] - self.tau_point[1]];
        let agreement = if norm(&c) < tol { dot(&b, &self.tau) } else { dot(&b, &c) * dot(&self.tau, &c) };
        let orientation = 1.0_f64.copysign(agreement);
        [orientation * b[0], orientation * b[1]]
    }

    /// The matrix whose determinant read() takes (None when the basis does not match the frame).
    fn sheet_matrix(&self, qr: &DragQr, tau: P2) -> Option<Vec<Vec<f64>>> {
        if qr.z.len() != self.reach + self.spare.len() { return None; }
        Some(qr.z.iter().map(|z| {
            let b = &z[self.point..self.point + 2];
            let mut row = match self.reach { 1 => vec![dot(b, &tau)], 2 => b.to_vec(), _ => Vec::new() };
            row.extend(self.spare.iter().map(|v| dot(z, v)));
            row
        }).collect())
    }

    /// Sum of the absolute entries of that matrix (the absolute cofactors beyond 2 by 2).
    fn sheet_scale(&self, qr: &DragQr, tau: P2) -> f64 {
        #[cfg(test)]
        super::work::add(|w| w.band_floor += 1);
        let Some(m) = self.sheet_matrix(qr, tau) else { return 0.0 };
        let k = m.len();
        if k <= 2 { return m.iter().flatten().map(|v| v.abs()).sum(); }
        let mut total = 0.0;
        for i in 0..k {
            for j in 0..k {
                let minor: Vec<Vec<f64>> = (0..k).filter(|&a| a != i).map(|a|
                    (0..k).filter(|&b| b != j).map(|b| m[a][b]).collect()).collect();
                total += determinant(minor).abs();
            }
        }
        total
    }

    fn read(&self, qr: &DragQr, tau: P2) -> SheetRead {
        let rank = qr.tau.len();
        let piv_sign: i32 = (0..rank).map(|i| sign(qr.columns[i][i])).product();
        // Replace entering pivots by the press rows they displaced. Orient the
        // resulting list against ascending S; retain the corresponding R^-1 C minor.
        let mut order = qr.order[..rank].to_vec();
        let mut minor = 1.0;
        if rank != self.pivot_rows.len() {
            minor = 0.0;
        } else if rank < qr.order.len() {
            let slots: Vec<_> = (0..rank).filter(|&i| !self.pivot_rows.contains(&order[i])).collect();
            let exchanged = slots.len();
            let missing: Vec<_> = self.pivot_rows.iter().copied().filter(|row| !order.contains(row)).collect();
            let mut patch = vec![vec![0.0; exchanged]; exchanged];
            for (j, &row) in missing.iter().enumerate() {
                let col = qr.order.iter().position(|&v| v == row).unwrap();
                let mut v = qr.columns[col][..rank].to_vec();
                for i in (0..rank).rev() {
                    v[i] /= qr.columns[i][i];
                    let w = v[i];
                    for k in 0..i { v[k] -= w * qr.columns[i][k]; }
                }
                for (i, &slot) in slots.iter().enumerate() { patch[i][j] = v[slot]; }
                order[slots[j]] = row;
            }
            minor = determinant(patch);
        }
        let mut parity = 1;
        for (j, &p) in order.iter().enumerate() {
            for &q in &order[..j] { if q > p { parity = -parity; } }
        }
        let omega = piv_sign * parity * sign(minor);
        let reach = qr.point_mobility(self.point);
        let det = if qr.z.len() != self.reach + self.spare.len() { 0.0 } else {
            determinant(qr.z.iter().map(|z| {
                let b = &z[self.point..self.point + 2];
                let mut row = match self.reach { 1 => vec![dot(b, &tau)], 2 => b.to_vec(), _ => Vec::new() };
                row.extend(self.spare.iter().map(|v| dot(z, v)));
                row
            }).collect())
        };
        SheetRead { reach, omega, det, sigma: omega * sign(det) }
    }
}

#[derive(Debug, Default)]
struct ProjectionCorrection {
    stored: f64,
    kept: bool,
    exact: bool,
}

/// Reuse the round's normals while the chord halves the largest residual.
fn project_pose(s: &System, x: &mut Vec<f64>, scale: f64, branches: &[f64], tol: f64,
                point: usize, correction: &mut ProjectionCorrection,
                round_qr: Option<&DragQr>) -> bool {
    chord_projection(s, x, scale, branches, tol, point, correction, round_qr, true)
}

fn chord_projection(s: &System, x: &mut Vec<f64>, scale: f64, branches: &[f64], tol: f64,
                    point: usize, correction: &mut ProjectionCorrection,
                    round_qr: Option<&DragQr>, allow_refactor: bool) -> bool {
    let n = x.len();
    let (mut r, mut jac, _) = s.evaluate(x, scale, branches);
    let mut residual = max_abs(&r);
    let mut fresh = round_qr.is_none();
    let mut local_qr = if fresh {
        #[cfg(test)]
        super::work::add(|w| w.projection_qr += 1);
        Some(DragQr::new(&jac, n))
    } else { None };
    while residual > 0.0 {
        let qr = local_qr.as_ref().or(round_qr).unwrap();
        let delta = qr.chord(&r, n);
        let next: Vec<_> = x.iter().zip(&delta).map(|(v, dv)| v + dv).collect();
        correction.stored = (next[point] - x[point]).hypot(next[point + 1] - x[point + 1]);
        let (next_r, next_jac, _) = s.evaluate(&next, scale, branches);
        let next_residual = max_abs(&next_r);
        let halved = next_residual <= 0.5 * residual;
        correction.kept = next_residual < residual;
        if correction.kept {
            *x = next;
            r = next_r;
            jac = next_jac;
            residual = next_residual;
            fresh = false;
        }
        if !halved {
            if residual <= tol { return true; }
            // A fresh factor at this same pose already failed to improve the
            // all-row residual. In particular a conflicting dependent row can
            // keep it nonzero after every pivot row has reached zero.
            if fresh || !allow_refactor { return false; }
            #[cfg(test)]
            super::work::add(|w| w.projection_qr += 1);
            local_qr = Some(DragQr::new(&jac, n));
            fresh = true;
        }
    }
    correction.exact = true;
    true
}

struct CurvatureTable {
    n: usize,
    h: f64,
    // For each Z direction, the nonzero Jacobian differences in each constraint row.
    diffs: Vec<Vec<Vec<(usize, f64)>>>,
}

fn row_differences(plus: &[(usize, f64)], minus: &[(usize, f64)]) -> Vec<(usize, f64)> {
    let mut row = Vec::new();
    let mut plus = plus.iter().copied().peekable();
    let mut minus = minus.iter().copied().peekable();
    loop {
        let a = match (plus.peek(), minus.peek()) {
            (Some(&(a, _)), Some(&(b, _))) => a.min(b),
            (Some(&(a, _)), None) | (None, Some(&(a, _))) => a,
            (None, None) => break,
        };
        let p = plus.next_if(|&(b, _)| b == a).map_or(0.0, |(_, value)| value);
        let m = minus.next_if(|&(b, _)| b == a).map_or(0.0, |(_, value)| value);
        let d = p - m;
        if d != 0.0 { row.push((a, d)); }
    }
    row
}

fn curvature_table(s: &System, qr: &DragQr, scale: f64, branches: &[f64], h: f64) -> CurvatureTable {
    #[cfg(test)]
    super::work::add(|w| w.hessian_tables += 1);
    let n = s.x.len();
    let mut diffs = Vec::with_capacity(qr.z.len());
    for zj in &qr.z {
        let plus: Vec<_> = s.x.iter().zip(zj).map(|(x, z)| x + h * z).collect();
        let minus: Vec<_> = s.x.iter().zip(zj).map(|(x, z)| x - h * z).collect();
        let (_, jp, _) = s.evaluate(&plus, scale, branches);
        let (_, jm, _) = s.evaluate(&minus, scale, branches);
        let rows = (0..jp.rows().min(jm.rows())).map(|i| row_differences(jp.row(i), jm.row(i))).collect();
        diffs.push(rows);
    }
    CurvatureTable { n, h, diffs }
}

fn reduced_hessian(table: &CurvatureTable, point: usize, qr: &DragQr, pull: P2) -> (Vec<Vec<f64>>, [f64; 2]) {
    let n = table.n;
    let h = table.h;
    let k = qr.z.len();
    #[cfg(test)]
    super::work::add(|w| w.reduced_hessian += (k as u64).pow(2) * n as u64);
    let mut rhs = vec![0.0; n];
    rhs[point..point + 2].copy_from_slice(&pull);
    let lambda = qr.multipliers(rhs);
    let mut hessian = vec![vec![0.0; k]; k];
    let mut point_squared = 0.0;
    let mut curvature_squared = 0.0;
    for (j, zj) in qr.z.iter().enumerate() {
        let mut curvature = vec![0.0; n];
        // Skipped terms add ±0 to a sum starting at +0. Exact cancellation rounds to +0,
        // so the sum never reaches −0, and the nonzero differences retain their original order.
        for (row, &l) in table.diffs[j].iter().zip(&lambda) {
            for &(a, d) in row {
                curvature[a] += l * d;
            }
        }
        for (i, zi) in qr.z.iter().enumerate() {
            let curvature_term = dot(zi, &curvature) / (2.0 * h);
            let point_term = dot(&zi[point..point + 2], &zj[point..point + 2]);
            point_squared += point_term * point_term;
            curvature_squared += curvature_term * curvature_term;
            hessian[i][j] = curvature_term + point_term;
        }
    }
    for j in 0..k {
        for i in 0..j {
            let a = (hessian[i][j] + hessian[j][i]) * 0.5;
            hessian[i][j] = a;
            hessian[j][i] = a;
        }
    }
    (hessian, [point_squared.sqrt(), curvature_squared.sqrt()])
}

struct SmallEigen {
    values: Vec<f64>,
    vectors: Vec<Vec<f64>>,
}

/// The reduced Hessian uses Jacobi through this size, which keeps every calibrated cell bit-exact,
/// and QL above it. The grams in `set_basis` and `reach_eigen` stay on Jacobi at every k. They
/// order a real eigenvalue against a zero cluster at `point_mobility`'s 1e-14 cut, where QL's
/// absolute error, about k·ε·‖G‖, is larger (Q28).
const JACOBI_MAX_K: usize = 16;

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum EigenSolver { Auto, Jacobi, Ql }

#[cfg(test)]
thread_local! {
    static EIGEN_SOLVER: std::cell::Cell<EigenSolver> = const { std::cell::Cell::new(EigenSolver::Auto) };
}

#[cfg(test)]
pub(crate) fn set_eigen_solver(solver: EigenSolver) -> EigenSolver {
    EIGEN_SOLVER.with(|slot| slot.replace(solver))
}

impl SmallEigen {
    fn new(a: Vec<Vec<f64>>) -> Self {
        #[cfg(test)]
        match EIGEN_SOLVER.with(std::cell::Cell::get) {
            EigenSolver::Jacobi => return Self::jacobi(a),
            EigenSolver::Ql => return Self::ql(a),
            EigenSolver::Auto => {}
        }
        if a.len() <= JACOBI_MAX_K { Self::jacobi(a) } else { Self::ql(a) }
    }

    fn jacobi(mut a: Vec<Vec<f64>>) -> Self {
        let k = a.len();
        #[cfg(test)]
        let mut count = 0u64;
        let magnitude = a.iter().map(|row| max_abs(row)).fold(0.0, f64::max);
        // Flat row-major storage preserves the Jacobi rotations bit for bit.
        let mut flat = Vec::with_capacity(k * k);
        for row in a.drain(..) {
            flat.extend_from_slice(&row);
        }
        let mut a = flat;
        let mut v = vec![0.0; k * k];
        for i in 0..k {
            v[i * k + i] = 1.0;
        }
        loop {
            let mut rotated = false;
            for i in 0..k {
                for j in i + 1..k {
                    let off = a[i * k + j];
                    if off.abs() <= f64::EPSILON * magnitude {
                        continue;
                    }
                    rotated = true;
                    // Two diagonal updates and two multiply-adds per rotated pair in A and V.
                    #[cfg(test)]
                    { count += 2 + 2 * ((k - 2) as u64 + k as u64); }
                    let half_gap = 0.5 * (a[j * k + j] - a[i * k + i]);
                    let t = if half_gap == 0.0 { 1.0 } else {
                        off / (half_gap + half_gap.hypot(off).copysign(half_gap))
                    };
                    let c = 1.0 / t.hypot(1.0);
                    let sn = t * c;
                    a[i * k + i] -= t * off;
                    a[j * k + j] += t * off;
                    a[i * k + j] = 0.0;
                    a[j * k + i] = 0.0;
                    for l in 0..k {
                        if l != i && l != j {
                            let (u, w) = (a[l * k + i], a[l * k + j]);
                            a[l * k + i] = c * u - sn * w;
                            a[l * k + j] = sn * u + c * w;
                            a[i * k + l] = a[l * k + i];
                            a[j * k + l] = a[l * k + j];
                        }
                        let (u, w) = (v[i * k + l], v[j * k + l]);
                        v[i * k + l] = c * u - sn * w;
                        v[j * k + l] = sn * u + c * w;
                    }
                }
            }
            if !rotated { break; }
        }
        let mut order: Vec<_> = (0..k).collect();
        order.sort_by(|&i, &j| a[i * k + i].total_cmp(&a[j * k + j]));
        #[cfg(test)]
        work::add(|w| w.eigen_jacobi_ops += count);
        Self {
            values: order.iter().map(|&i| a[i * k + i]).collect(),
            vectors: order.iter().map(|&i| v[i * k..(i + 1) * k].to_vec()).collect(),
        }
    }

    /// Symmetric Householder reduction followed by implicit QL on the tridiagonal matrix.
    fn ql(a: Vec<Vec<f64>>) -> Self {
        let k = a.len();
        let mut matrix = Vec::with_capacity(k * k);
        for row in a {
            matrix.extend_from_slice(&row);
        }
        let mut q = vec![0.0; k * k];
        for i in 0..k { q[i * k + i] = 1.0; }
        let mut e = vec![0.0; k];
        let mut work_vec = vec![0.0; k];
        #[cfg(test)]
        let mut count = 0u64;

        // Reduce row i to its subdiagonal entry. Earlier rows form the active symmetric block A.
        for i in (2..k).rev() {
            let (leading, rest) = matrix.split_at_mut(i * k);
            let u = &mut rest[..i];
            let scale: f64 = u.iter().map(|x| x.abs()).sum();
            if scale == 0.0 { continue; }
            for x in &mut *u { *x /= scale; }
            let squared = dot(u, u);
            let alpha = -squared.sqrt().copysign(u[i - 1]);
            let beta = 1.0 / (squared - alpha * u[i - 1]);
            u[i - 1] -= alpha;
            e[i - 1] = scale * alpha;

            // H = I - βuuᵀ. Write HAH as A - uwᵀ - wuᵀ, updating each symmetric pair once.
            let w = &mut work_vec[..i];
            for (wj, row) in w.iter_mut().zip(leading.chunks_exact(k)) {
                *wj = beta * dot(&row[..i], u);
            }
            let correction = 0.5 * beta * dot(u, w);
            for j in 0..i { w[j] -= correction * u[j]; }
            for j in 0..i {
                for l in 0..=j {
                    let value = leading[j * k + l] - (u[j] * w[l] + w[j] * u[l]);
                    leading[j * k + l] = value;
                    leading[l * k + j] = value;
                }
            }
            // Accumulate QH in reduction order, so the tridiagonal is Qᵀ A_original Q.
            for row in q.chunks_exact_mut(k) {
                let weight = beta * dot(&row[..i], u);
                for l in 0..i { row[l] -= weight * u[l]; }
            }
            #[cfg(test)]
            {
                let i = i as u64;
                count += 2 * i * i + 4 * i + 1 + 2 * k as u64 * i;
            }
        }
        // e[i] couples d[i] to d[i+1]. The final entry remains the zero sentinel.
        if k > 1 { e[0] = matrix[k]; }
        let mut d: Vec<_> = (0..k).map(|i| matrix[i * k + i]).collect();

        let mut tst1 = 0.0_f64;
        for l in 0..k {
            tst1 = tst1.max(d[l].abs() + e[l].abs());
            let cut = f64::EPSILON * tst1;
            'sweep: loop {
                let mut m = l;
                while m + 1 < k && e[m].abs() > cut { m += 1; }
                if m == l { break; }

                // Wilkinson shift of the leading 2 × 2, carried implicitly from the far end.
                let gap = (d[l + 1] - d[l]) / (2.0 * e[l]);
                let mut g = (d[m] - d[l]) + e[l] / (gap + gap.hypot(1.0).copysign(gap));
                let (mut sine, mut cosine, mut shift) = (1.0, 1.0, 0.0);
                for i in (l..m).rev() {
                    let f = sine * e[i];
                    let b = cosine * e[i];
                    let radius = g.hypot(f);
                    e[i + 1] = radius;
                    if radius == 0.0 {
                        d[i + 1] -= shift;
                        e[m] = 0.0;
                        continue 'sweep;
                    }
                    sine = f / radius;
                    cosine = g / radius;
                    g = d[i + 1] - shift;
                    let delta = (d[i] - g) * sine + 2.0 * cosine * b;
                    shift = sine * delta;
                    d[i + 1] = g + shift;
                    g = cosine * delta - b;
                    for row in q.chunks_exact_mut(k) {
                        let (x, y) = (row[i], row[i + 1]);
                        row[i] = cosine * x - sine * y;
                        row[i + 1] = sine * x + cosine * y;
                    }
                    #[cfg(test)]
                    { count += 3 + 2 * k as u64; }
                }
                d[l] -= shift;
                e[l] = g;
                e[m] = 0.0;
            }
        }
        let mut order: Vec<_> = (0..k).collect();
        order.sort_by(|&i, &j| d[i].total_cmp(&d[j]));
        #[cfg(test)]
        work::add(|w| w.eigen_ql_ops += count);
        Self {
            values: order.iter().map(|&j| d[j]).collect(),
            vectors: order.iter().map(|&j| (0..k).map(|i| q[i * k + j]).collect()).collect(),
        }
    }

    /// Solve the trust-region secular equation in the eigenbasis. Measure
    /// the shift from its lower bound so a nearly hard case retains precision.
    fn step(&self, gradient: &[f64], radius: f64, floor: f64, slope_floor: f64, tol: f64,
            z: &[Vec<f64>], x: &[f64], anchor: &[f64], other_side: bool, unzero: &[bool]) -> (Vec<f64>, bool, Option<FoldChoice>) {
        let ev: Vec<_> = self.values.iter().enumerate().map(|(j, &v)| if v.abs() <= floor && !unzero.get(j).copied().unwrap_or(false) { 0.0 } else { v }).collect();
        let mut g: Vec<_> = self.vectors.iter().map(|v| dot(v, gradient)).collect();
        let mut hard_sign = 1.0;
        let mut fold_choice = None;
        for (j, (gj, &value)) in g.iter_mut().zip(&ev).enumerate() {
            if value == 0.0 && gj.abs() <= slope_floor {
                *gj = 0.0;
            } else if value < 0.0 && gj.abs() <= value.abs() * tol {
                *gj = 0.0;
                // Only the minimum eigenspace can receive the hard-case fill.
                if j == 0 {
                    let mut toward_anchor = 0.0;
                    let mut direction = Vec::new();
                    for a in 0..x.len() {
                        let dir: f64 = self.vectors[j].iter().zip(z).map(|(v, col)| v * col[a]).sum();
                        toward_anchor += dir * (anchor[a] - x[a]);
                        direction.push(dir);
                    }
                    let largest = max_abs(&direction);
                    let threshold = 0.5 * largest;
                    let convention = direction.iter().find(|v| v.abs() >= threshold).unwrap().signum();
                    hard_sign = if toward_anchor.abs() > tol { toward_anchor.signum() } else { convention };
                    if other_side { hard_sign = -hard_sign; }
                    fold_choice = Some(FoldChoice { convention, sign: hard_sign });
                }
            }
        }
        let lower = (-ev[0]).max(0.0);
        let mut hard = true;
        let mut y = vec![0.0; g.len()];
        for j in 0..g.len() {
            let shifted = ev[j] + lower;
            if shifted > 0.0 {
                y[j] = -g[j] / shifted;
            } else if g[j].abs() > f64::EPSILON * norm(&g) {
                hard = false;
            }
        }
        let mut boundary = false;
        if hard && norm(&y) <= radius {
            if lower > 0.0 {
                y[0] = hard_sign * (radius * radius - dot(&y, &y)).max(0.0).sqrt();
                boundary = true;
            }
        } else {
            boundary = true;
            let mut lo = 0.0;
            let mut hi = norm(&g) / radius;
            loop {
                let mid = lo + 0.5 * (hi - lo);
                if mid == lo || mid == hi { break; }
                for j in 0..y.len() {
                    y[j] = -g[j] / ((ev[j] + lower) + mid);
                }
                if norm(&y) > radius { lo = mid; } else { hi = mid; }
            }
            for j in 0..y.len() {
                y[j] = -g[j] / ((ev[j] + lower) + hi);
            }
        }
        let mut step = vec![0.0; g.len()];
        for (coef, v) in y.iter().zip(&self.vectors) {
            for (s, u) in step.iter_mut().zip(v) {
                *s += coef * u;
            }
        }
        (step, boundary, fold_choice)
    }
}

#[derive(Debug)]
struct FoldChoice {
    convention: f64,
    sign: f64,
}

fn drag_gradient(qr: &DragQr, point: usize, d: P2) -> Vec<f64> {
    qr.z.iter().map(|v| -dot(&v[point..point + 2], &d)).collect()
}

fn reach_eigen(qr: &DragQr, point: usize) -> SmallEigen {
    let gram = qr.z.iter().map(|a| qr.z.iter().map(|b|
        dot(&a[point..point+2], &b[point..point+2])).collect()).collect();
    SmallEigen::jacobi(gram)
}

fn least_mobile_direction(qr: &DragQr, point: usize) -> Vec<f64> {
    reach_eigen(qr, point).vectors.remove(0)
}

/// Reflect along the least mobile right singular vector of P Z. In this
/// direction the two model steps have the same quadratic objective.
fn mirror_step(qr: &DragQr, point: usize, hessian: &[Vec<f64>], gamma: &[f64],
               step: &[f64], floor: f64) -> Vec<f64> {
    let w = least_mobile_direction(qr, point);
    let hw: Vec<_> = hessian.iter().map(|row| dot(row, &w)).collect();
    let curvature = dot(&w, &hw);
    let fallback = curvature.abs() <= floor;
    let offset = if fallback { dot(&w, step) }
                 else { (dot(&w, gamma) + dot(&hw, step)) / curvature };
    let mirror = step.iter().zip(&w).map(|(v, wi)| v - 2.0 * offset * wi).collect();
    mirror
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum TrialKind { Model, Mirror, Twin, Ray }

struct Twin {
    seed: Vec<f64>,
    qr: DragQr,
}

enum TwinAttempt { Ready(Twin), Rounding, Overlength, ProbeFailed }

struct Refusal {
    step: Vec<f64>,
    chord: SheetChord,
    twin: Option<Twin>,
}

fn along_fibre(pose: &[f64], fibre: &[f64], length: f64) -> Vec<f64> {
    pose.iter().zip(fibre).map(|(x,w)| x + length*w).collect()
}

fn make_twin(s: &System, qr: &DragQr, trial: &[f64], trial_qr: DragQr, frame: &PressFrame,
             chord: SheetChord, scale: f64, tol: f64) -> TwinAttempt {
    let i = frame.point;
    let tip = [trial[i], trial[i+1]];
    let eig = reach_eigen(&trial_qr, i);
    // Eigenvectors of BᵀB are in ascending order. Skip the spare directions.
    let w = &eig.vectors[eig.vectors.len()-frame.reach];
    let fibre: Vec<f64> = (0..trial.len()).map(|j|
        trial_qr.z.iter().zip(w).map(|(z,v)| z[j]*v).sum()).collect();
    let m = fibre[i].hypot(fibre[i+1]);
    let singular_value = |at: &DragQr, w: &[f64]| {
        let component = |j| at.z.iter().zip(w).map(|(z,v): (&Vec<f64>, &f64)| z[j]*v).sum::<f64>();
        component(i).hypot(component(i+1))
    };
    let at_start=reach_eigen(qr,i);
    let m_x=singular_value(qr,&at_start.vectors[at_start.vectors.len()-frame.reach]);
    let h = chord.length*m/(m_x+m);
    let mut y = along_fibre(trial, &fibre, h);
    let projected = chord_projection(s, &mut y, scale, &frame.branches, tol,
        i, &mut ProjectionCorrection::default(), Some(&trial_qr), false);
    if !projected {
        return TwinAttempt::ProbeFailed;
    }
    let a = [fibre[i]/m, fibre[i+1]/m];
    let u = dot(&a, &[y[i]-tip[0], y[i+1]-tip[1]]);
    let remainder = u-m*h;
    let kappa = 2.0*remainder/(h*h);
    let beta = -2.0*m/kappa;
    let rounding = 8.0*f64::EPSILON*max_abs(&tip).max(1.0);
    if !h.is_finite() || h == 0.0 || m == 0.0 {
        return TwinAttempt::ProbeFailed;
    }
    if remainder.abs() <= rounding || beta.abs() > chord.length {
        let unreadable = remainder.abs() <= rounding;
        if unreadable {
            return TwinAttempt::Rounding;
        }
        return TwinAttempt::Overlength;
    }
    TwinAttempt::Ready(Twin { seed: along_fibre(trial, &fibre, beta), qr: trial_qr })
}

struct RayCut {
    ok: f64,
    past: f64,
    d_past: f64,
    secant_ok: f64,
    bisect: bool,
    kept_ok: bool,
    t: f64,
}

impl RayCut {
    fn new(chord: SheetChord) -> Self {
        Self { ok: 0.0, past: 1.0, d_past: chord.after,
               secant_ok: chord.before, bisect: false, kept_ok: false, t: chord.alpha }
    }

    fn advance(&mut self, d: f64) {
        if d < 0.0 && d < self.d_past { self.bisect = true; }
        self.past = self.t;
        self.d_past = d;
        if self.kept_ok { self.secant_ok *= 0.5; }
        self.kept_ok = true;
        self.t = if self.bisect { 0.5*(self.ok+self.past) }
            else { self.ok + (self.past-self.ok)*self.secant_ok/(self.secant_ok-self.d_past) };
        if !(self.t > self.ok && self.t < self.past) { self.t = 0.5*(self.ok+self.past); }
    }
}

/// One weak direction judged on the reduced model (Q19 amendment, section 3).
#[derive(Clone, Debug, Default)]
struct WeakInfo {
    j: usize,
    settled: bool,
    ridge: bool,
}

#[derive(Clone, Debug, Default)]
struct Reduced {
    r_norm: f64,
    weak: Vec<WeakInfo>,
    /// Any direction below -c0, or a weak ridge.
    ridge_any: bool,
    all_settled: bool,
}

/// The reduced model per weak direction. Sign: c is such that r . c = v^T K_d v, so that
/// with S empty r.c = ev - |Bv|^2.
fn reduced_model(table: &CurvatureTable, point: usize, qr: &DragQr, eig: &SmallEigen, r: P2, tol: f64, nu: f64,
                 c0: f64, norm_btb: f64) -> Reduced {
    let k = qr.z.len();
    let bcol: Vec<P2> = qr.z.iter().map(|z| [z[point], z[point + 1]]).collect();
    let strong: Vec<usize> = (0..k).filter(|&j| eig.values[j] > c0).collect();
    let weak_ids: Vec<usize> = (0..k).filter(|&j| eig.values[j].abs() <= c0).collect();
    let apply = |v: &[f64]| -> P2 { let mut o = [0.0, 0.0]; for (i, b) in bcol.iter().enumerate() { o[0] += v[i] * b[0]; o[1] += v[i] * b[1]; } o };
    let cols: Vec<P2> = strong.iter().map(|&j| apply(&eig.vectors[j])).collect();
    let big = cols.iter().map(|c| c[0].hypot(c[1])).fold(0.0, f64::max);
    let mut q: Vec<P2> = Vec::new();
    for c in &cols {
        let mut v = *c;
        for u in &q { let d = v[0] * u[0] + v[1] * u[1]; v[0] -= d * u[0]; v[1] -= d * u[1]; }
        let n = v[0].hypot(v[1]);
        if big > 0.0 && n > RANK_CUT * big { q.push([v[0] / n, v[1] / n]); }
    }
    let project = |x: P2| -> P2 { let mut o = x; for u in &q { let d = x[0] * u[0] + x[1] * u[1]; o[0] -= d * u[0]; o[1] -= d * u[1]; } o };
    let rank_s = q.len();
    let rperp_v = project(r);
    let r_norm = r[0].hypot(r[1]);
    let rperp = rperp_v[0].hypot(rperp_v[1]);
    let tau = tol / 2.0;
    let beta = 0.5 * tau * r_norm.max(tau);
    let mut out = Reduced { r_norm, weak: Vec::new(), ridge_any: eig.values.iter().any(|&e| e < -c0), all_settled: true };
    if weak_ids.is_empty() { return out; }
    let (kx, ky) = if rank_s < 2 {
        let btb = |i: usize, j: usize| bcol[i][0] * bcol[j][0] + bcol[i][1] * bcol[j][1];
        let hx = reduced_hessian(table, point, qr, [1.0, 0.0]).0;
        let hy = reduced_hessian(table, point, qr, [0.0, 1.0]).0;
        let sub = |m: &Vec<Vec<f64>>| (0..k).map(|i| (0..k).map(|j| m[i][j] - btb(i, j)).collect::<Vec<f64>>()).collect::<Vec<_>>();
        (sub(&hx), sub(&hy))
    } else { (Vec::new(), Vec::new()) };
    for &j in &weak_ids {
        let v = &eig.vectors[j];
        let a = project(apply(v));
        let a2 = a[0] * a[0] + a[1] * a[1];
        let (c, cperp_v) = if rank_s < 2 {
            let quad = |m: &Vec<Vec<f64>>| (0..k).map(|i| (0..k).map(|l| v[i] * m[i][l] * v[l]).sum::<f64>()).sum::<f64>();
            let c = [-quad(&kx), -quad(&ky)];
            (c, project(c))
        } else { ([0.0, 0.0], [0.0, 0.0]) };
        let c_norm = c[0].hypot(c[1]);
        let rc = rperp_v[0] * cperp_v[0] + rperp_v[1] * cperp_v[1];
        let g_eff = rperp_v[0] * a[0] + rperp_v[1] * a[1];
        let ev_eff = a2 + rc;
        let n_eff = 2e-8 * rperp * c_norm + 16.0 * f64::EPSILON * norm_btb;
        let dec = g_eff * g_eff / (2.0 * ev_eff.max(n_eff));
        let settled = g_eff.abs() <= nu || dec <= beta;
        let ridge = ev_eff < -n_eff && g_eff.abs() > nu;
        out.ridge_any |= ridge;
        out.all_settled &= settled;
        out.weak.push(WeakInfo { j, settled, ridge });
    }
    out
}

/// How one move ended: its rounds and its outcome.
pub(super) struct MoveEnd {
    pub(super) rounds: usize,
    pub(super) outcome: DragOutcome,
}

/// The pose a move ended at, kept for the next move's start. Its residuals `r` and compressed
/// Jacobian `jac` are exactly those from `s.evaluate(&pose, scale, &frame.branches)`, and
/// `qr` is exactly `DragQr::new(&jac, pose.len())`.
#[derive(Clone, Debug)]
pub(super) struct Kept {
    pose: Vec<f64>,
    scale: f64,
    r: Vec<f64>,
    jac: Jac,
    qr: DragQr,
}

/// Constraint residual, slope and curvature used to judge one move, and its stopping state.
struct MoveState {
    tol: f64,
    rounds: usize,
    residual: f64,
    gamma: f64,
    nu: f64,
    lambda_min: f64,
    outcome: DragOutcome,
}

/// An accepted pose holds the constraints and is no farther from the target
/// within the distance objective's measured resolution. Where a sheet can
/// be enforced, accepted poses stay on the press sheet or in its fold band.
/// Reuse `kept` when pose and scale match bit for bit, and keep the end pose's evaluation and QR when available.
pub(super) fn drag_move(s: &mut System, p: PointId, target: P2, opts: &Options, frame: &mut PressFrame,
                        scale: f64, kept: &mut Option<Kept>) -> MoveEnd {
    let previous = kept.take().filter(|entry| {
        entry.scale.to_bits() == scale.to_bits() && entry.pose.len() == s.x.len()
            && entry.pose.iter().zip(&s.x).all(|(a, b)| a.to_bits() == b.to_bits())
    });
    let start = s.x.clone();
    let start_frame = frame.clone();
    let n = s.x.len();
    let i = s.points[p.0];
    let branches = frame.branches.clone();
    let h = f64::EPSILON.cbrt() * scale;
    let mut report = MoveState {
        tol: opts.tolerance * scale, rounds: 0, residual: 0.0, gamma: 0.0, nu: 0.0,
        lambda_min: 0.0, outcome: DragOutcome::NotReached,
    };
    let (mut r, mut jac, mut cached_qr) = match previous {
        Some(entry) => (entry.r, entry.jac, Some(entry.qr)),
        None => {
            let (r, jac, _) = s.evaluate(&s.x, scale, &branches);
            (r, jac, None)
        }
    };
    if max_abs(&r) > report.tol {
        cached_qr = None;
        let mut x = s.x.clone();
        let mut correction = ProjectionCorrection::default();
        let projected = project_pose(s, &mut x, scale, &branches, report.tol, i, &mut correction, None);
        if !projected {
            return MoveEnd { rounds: 0, outcome: DragOutcome::NotReached };
        }
        s.x = x;
        (r, jac, _) = s.evaluate(&s.x, scale, &branches);
    }
    let mut radius = f64::INFINITY;
    let mut best_gamma_accept = f64::INFINITY;
    let mut restored_start = false;
    // Each round carries forward the trust radius, frozen constraints and acceptance history.
    let mut advance_one_round = || -> bool {
        if report.rounds >= opts.max_iterations {
            eprintln!("constrain drag: the move stopped at {} rounds (Options::max_iterations); it keeps its last accepted pose", report.rounds);
            return false;
        }
        report.rounds += 1;
        let qr: &DragQr = cached_qr.get_or_insert_with(|| {
            #[cfg(test)]
            super::work::add(|w| w.start_qr += 1);
            DragQr::new(&jac, n)
        });
        // Q26: the spare is independent of the fold when curvature and slope vanish on every spare
        // direction at both axis pulls, scale·e_x and scale·e_y. This pull-free test gives the press
        // its class independently of the first target. Judge it in the frame's first round, then freeze it.
        let mut judged_table = None;
        if !frame.spare.is_empty() && !frame.spare_judged {
            let table = curvature_table(s, qr, scale, &branches, h);
            let k = qr.z.len();
            let btb = |a: usize, b: usize| dot(&qr.z[a][i..i + 2], &qr.z[b][i..i + 2]);
            let (hx, nrm) = reduced_hessian(&table, i, qr, [1.0, 0.0]);
            let hy = reduced_hessian(&table, i, qr, [0.0, 1.0]).0;
            let p_norm = nrm[0];
            let kappa = qr.noise_floor(&jac, scale);
            let mut ok = true;
            for v in &frame.spare {
                let u: Vec<f64> = qr.z.iter().map(|z| dot(z, v)).collect();
                let quad = |m: &Vec<Vec<f64>>| (0..k).map(|a| (0..k).map(|b| u[a] * (m[a][b] - btb(a, b)) * u[b]).sum::<f64>()).sum::<f64>();
                let c = [-quad(&hx), -quad(&hy)];
                let n_eff = 2e-8 * scale * c[0].hypot(c[1]) + 16.0 * f64::EPSILON * p_norm;
                for (a_dir, h_a) in [(0usize, &hx), (1usize, &hy)] {
                    let w: Vec<f64> = (0..k).map(|p| (0..k).map(|q| btb(p, q) * u[q] + scale * (h_a[p][q] - btb(p, q)) * u[q]).sum::<f64>()).collect();
                    let g_a = scale * (0..k).map(|j| u[j] * qr.z[j][i + a_dir]).sum::<f64>();
                    if norm(&w) > n_eff || g_a.abs() > kappa { ok = false; }
                }
            }
            frame.independent_spare = ok;
            frame.spare_judged = true;
            judged_table = Some(table);
        }
        // R8: check Reached before building the round's model.
        report.residual = max_abs(&r);
        let offset = [s.x[i] - target[0], s.x[i + 1] - target[1]];
        let length = offset[0].hypot(offset[1]);
        if report.residual <= report.tol && length <= report.tol / 2.0 {
            report.outcome = DragOutcome::Reached;
            return false;
        }
        let d = [target[0] - s.x[i], target[1] - s.x[i + 1]];
        let distance = norm(&d);
        let coordinate_floor = 8.0 * f64::EPSILON * max_abs(&s.x).max(1.0) * distance;
        let gamma = drag_gradient(qr, i, d);
        report.gamma = norm(&gamma);
        report.nu = qr.noise_floor(&jac, distance);
        let table = judged_table.take().unwrap_or_else(|| curvature_table(s, qr, scale, &branches, h));
        let (hessian, term_norms) = reduced_hessian(&table, i, qr, d);
        let eig = SmallEigen::new(hessian.clone());
        report.lambda_min = eig.values.first().copied().unwrap_or(0.0);
        let curvature_floor = CURVATURE_FLOOR * (term_norms[0] + term_norms[1]);
        let g: Vec<_> = eig.vectors.iter().map(|v| dot(v, &gamma)).collect();
        let slope_floor = report.tol.max(report.nu);
        let flat = report.lambda_min >= -curvature_floor && !g.iter().zip(&eig.values)
            .any(|(gj, ev)| ev.abs() <= curvature_floor && gj.abs() > slope_floor);
        let positive_newton_length = g.iter().zip(&eig.values)
            .filter(|(_, ev)| **ev > curvature_floor)
            .map(|(gj, ev)| (gj / ev).powi(2)).sum::<f64>().sqrt();
        let r_vec = [s.x[i] - target[0], s.x[i + 1] - target[1]];
        let red = reduced_model(&table, i, qr, &eig, r_vec, report.tol, report.nu, curvature_floor, term_norms[0]);
        let pin_early = {
            let mut by_e = [0.0_f64, 0.0];
            for j in 0..qr.z.len() {
                if eig.values[j] > curvature_floor {
                    let yj = g[j] / eig.values[j];
                    for (a, u) in eig.vectors[j].iter().enumerate() { by_e[0] += yj * u * qr.z[a][i]; by_e[1] += yj * u * qr.z[a][i + 1]; }
                }
            }
            by_e[0].hypot(by_e[1])
        };
        let nearest_new = report.residual <= report.tol && !red.ridge_any && pin_early <= report.tol * (2.0 / 3.0) && red.all_settled;
        // Q22 rule (b): a start below the frame's rank with a strong negative direction takes
        // the quartic radius θ* when the gates hold (−ev <= 1/32, the linear share <= 1/8, θ* <= scale/4).
        let toggle_candidate = frame.current.reach < frame.reach && report.lambda_min < -curvature_floor;
        let pin_inf = s.x[i].abs().max(s.x[i + 1].abs());
        if toggle_candidate {
            let k = qr.z.len();
            let v = &eig.vectors[0];
            let btb = |a: usize, b: usize| dot(&qr.z[a][i..i + 2], &qr.z[b][i..i + 2]);
            let hx = reduced_hessian(&table, i, qr, [1.0, 0.0]).0;
            let hy = reduced_hessian(&table, i, qr, [0.0, 1.0]).0;
            let quad = |m: &Vec<Vec<f64>>| (0..k).map(|a| (0..k).map(|b| v[a] * (m[a][b] - btb(a, b)) * v[b]).sum::<f64>()).sum::<f64>();
            let c = [-quad(&hx), -quad(&hy)];
            let c_norm = c[0].hypot(c[1]);
            let r_dot_c = r_vec[0] * c[0] + r_vec[1] * c[1];
            let theta = if r_dot_c < 0.0 { (-2.0 * r_dot_c / (c_norm * c_norm)).sqrt() } else { f64::NAN };
            let pred_q_round = if c_norm > 0.0 { eig.values[0] * eig.values[0] / (2.0 * c_norm * c_norm) } else { f64::NAN };
            let lin_share = report.gamma * theta / pred_q_round;
            let ev_gate = -eig.values[0] <= 1.0 / 32.0;
            // Q25 note 2: the quartic expansion holds over a fraction of the sketch, not over many sketch
            // sizes, so the radius it routes is at most scale/4 (g1's bound with |c| at the sketch's 1/scale).
            let gate_b = theta.is_finite() && lin_share <= 0.125 && ev_gate && theta <= scale / 4.0;
            if gate_b {
                radius = theta;
            }
        }

        // Q23 section 4: rule (b) with the offset phi0, for a start at the frame's rank that sits phi0 off a toggle.
        // `lambda_min` is the first eigenvalue, or 0 with no free direction, so a fully constrained frame skips the offset rule.
        if !toggle_candidate && frame.current.reach == frame.reach && report.lambda_min < -curvature_floor {
            let k = qr.z.len();
            let v = &eig.vectors[0];
            let btb = |a: usize, b: usize| dot(&qr.z[a][i..i + 2], &qr.z[b][i..i + 2]);
            let hx = reduced_hessian(&table, i, qr, [1.0, 0.0]).0;
            let hy = reduced_hessian(&table, i, qr, [0.0, 1.0]).0;
            let quad = |m: &Vec<Vec<f64>>| (0..k).map(|a| (0..k).map(|b| v[a] * (m[a][b] - btb(a, b)) * v[b]).sum::<f64>()).sum::<f64>();
            let c = [-quad(&hx), -quad(&hy)];
            let c2 = c[0] * c[0] + c[1] * c[1];
            let bv = [(0..k).map(|j| v[j] * qr.z[j][i]).sum::<f64>(), (0..k).map(|j| v[j] * qr.z[j][i + 1]).sum::<f64>()];
            if c2 > 0.0 {
                let c_norm_o = c2.sqrt();
                let phi0 = (bv[0] * c[0] + bv[1] * c[1]) / c2;
                let r_c = r_vec[0] * c[0] + r_vec[1] * c[1];
                let a_t = r_c - 0.5 * c2 * phi0 * phi0;
                let phi2 = -2.0 * a_t / c2;
                let phi_star = if phi2 > 0.0 { phi2.sqrt() } else { f64::NAN };
                let sgn = if phi0 >= 0.0 { 1.0 } else { -1.0 };
                let theta_near = sgn * phi_star - phi0;
                let pred_q_o = r_c * r_c / (2.0 * c2);
                let gres_vec: Vec<f64> = gamma.iter().zip(v).map(|(gg, vv)| gg - r_c * phi0 * vv).collect();
                let bres = [bv[0] - phi0 * c[0], bv[1] - phi0 * c[1]];
                let g1 = a_t < 0.0 && -a_t <= 1.0 / 32.0;
                let g2 = norm(&gres_vec) * theta_near.abs() <= pred_q_o / 8.0;
                let g3 = bres[0].hypot(bres[1]) * theta_near.abs() <= r_c.abs() / (8.0 * c_norm_o);
                let gate_o = phi_star.is_finite() && g1 && g2 && g3 && theta_near.abs() <= scale / 4.0;
                if gate_o {
                    radius = theta_near.abs();
                }
            }
        }
        // R8: the target is reached to within tol/2 and the constraints hold.
        let reached = report.residual <= report.tol && red.r_norm <= report.tol / 2.0;
        let stop_now = nearest_new || reached;
        if stop_now {
            report.outcome = if reached { DragOutcome::Reached } else { DragOutcome::Nearest };
            return false;
        }
        if radius.is_infinite() && !flat {
            radius = distance.max(positive_newton_length);
        }
        // Reuse the model at this pose when a trial shrinks the radius.
        let mut other_side = false;
        let mut first_side = None;
        let regular = frame.spare.is_empty() || frame.independent_spare;
        let mut kind = TrialKind::Model;
        let mut refused: Option<Refusal> = None;
        let mut ray: Option<RayCut> = None;
        // Q17: a round that starts at D <= 0, or below the frame's reach rank.
        let start_d = f64::from(frame.press_sign * frame.current.omega) * frame.current.det;
        let fold_start = start_d <= 0.0 || frame.current.reach < frame.reach;
        let mut fold_level = 0usize;
        // Q18 section 3: the radius the round started with, handed back after a clean fold shrink.
        let round_radius = radius;
        let mut round_clean = true;
        // The Q17 handler: the mirror was not accepted, or did not project. A fold
        // start shrinks the radius and tries the model and its mirror again.
        macro_rules! fold_shrink { () => {{
            let source = refused.as_ref().unwrap();
            let model_len = norm(&source.step);
            let next = SHRINK * model_len;
            fold_level += 1;
            radius = next;
            ray = None;
            refused = None;
            kind = TrialKind::Model;
            other_side = false;
            first_side = None;
            if radius < f64::EPSILON * scale {
                return false;
            }
            continue;
        }}}

        let unzero: Vec<bool> = (0..qr.z.len()).map(|j| red.weak.iter().any(|w| w.j == j && !w.settled && !w.ridge)).collect();
        let mut first_len = 0.0_f64;
        let mut trial_iter = 0usize;
        let mut r_u: Option<f64> = None;
        loop {
            trial_iter += 1;
            if trial_iter > frame.trial_cap {
                eprintln!("constrain drag: a round reached {} trials; the move keeps its last accepted pose", frame.trial_cap);
                return false;
            }
            let (original, mut boundary, fold_choice) = eig.step(&gamma, radius, curvature_floor, slope_floor, report.tol, &qr.z, &s.x, &frame.anchor, other_side, &unzero);
            let original_length = norm(&original);
            let mut mirror_neg = false;
            let mirror = (kind == TrialKind::Mirror).then(|| mirror_step(qr, i, &hessian, &gamma, &refused.as_ref().unwrap().step, curvature_floor));
            if kind == TrialKind::Mirror && fold_start {
                let m = mirror.as_ref().unwrap();
                let model = &refused.as_ref().unwrap().step;
                let apart = m.iter().zip(model).map(|(a, b)| (a - b).powi(2)).sum::<f64>().sqrt();
                let model_len = norm(model);
                if apart <= 8.0 * f64::EPSILON * model_len {
                    // An identity mirror. With sᵀHs < 0 and an even model along s (odd share <= 1/8), the
                    // trial is −s at the same radius (MIRROR_NEG, Q21, Q22c part 3). Otherwise the round shrinks (B2).
                    let gs = dot(&gamma, model);
                    let hs_all: Vec<f64> = hessian.iter().map(|row| dot(row, model)).collect();
                    let shs = dot(model, &hs_all);
                    let odd_share = if shs.abs() > 0.0 { gs.abs() / (0.5 * shs.abs()) } else if gs == 0.0 { 0.0 } else { f64::INFINITY };
                    if shs < 0.0 && odd_share <= 0.125 {
                        mirror_neg = true;
                    } else {
                        fold_shrink!();
                    }
                }
            }
            let mut projection_qr = qr;
            let mut x = s.x.clone();
            let step = match kind {
                TrialKind::Model => original.clone(),
                TrialKind::Mirror => if mirror_neg { refused.as_ref().unwrap().step.iter().map(|v| -v).collect() } else { mirror.as_ref().unwrap().clone() },
                TrialKind::Ray => {
                    boundary = false;
                    let cut=ray.as_ref().unwrap();
                    let t=cut.t;
                    refused.as_ref().unwrap().step.iter().map(|v| t*v).collect()
                }
                TrialKind::Twin => {
                    let twin = refused.as_ref().unwrap().twin.as_ref().unwrap();
                    x.clone_from(&twin.seed);
                    projection_qr = &twin.qr;
                    // The objective model lives at the round's x. Express this
                    // ambient trial displacement in that model's tangent basis.
                    let dx: Vec<_> = x.iter().zip(&s.x).map(|(a,b)| a-b).collect();
                    qr.z.iter().map(|z| dot(z,&dx)).collect()
                }
            };
            let mut step = step;
            if fold_start && !frame.spare.is_empty() && matches!(kind, TrialKind::Model | TrialKind::Mirror) {
                let mut amb = vec![0.0; qr.z[0].len()];
                for (z, sj) in qr.z.iter().zip(&step) { for (a, zi) in amb.iter_mut().zip(z) { *a += sj * zi; } }
                let mut s_v = vec![0.0; amb.len()];
                for v in &frame.spare { let c = dot(&amb, v); for (o, vi) in s_v.iter_mut().zip(v) { *o += c * vi; } }
                let ratio = norm(&s_v) / norm(&amb).max(f64::MIN_POSITIVE);
                if ratio > 8.0 * f64::EPSILON {
                    let mut stripped = step.clone();
                    for (sj, z) in stripped.iter_mut().zip(&qr.z) { *sj -= dot(z, &s_v); }
                    let prediction = |v: &[f64]| {
                        let quadratic: f64 = hessian.iter().zip(v).map(|(row, si)| si * dot(row, v)).sum();
                        -dot(&gamma, v) - 0.5 * quadratic
                    };
                    // The strip may spend at most 1/8 of this trial's predicted gain.
                    if prediction(&stripped) >= (7.0 / 8.0) * prediction(&step) {
                        step = stripped;
                    } else {
                        #[cfg(test)]
                        super::work::add(|w| w.strip_undone += 1);
                    }
                }
            }
            if kind != TrialKind::Twin {
                for (v, sj) in qr.z.iter().zip(&step) {
                    for (xa, za) in x.iter_mut().zip(v) { *xa += sj * za; }
                }
            }
            let length = norm(&step);
            let quadratic: f64 = hessian.iter().zip(&step).map(|(row, si)| si * dot(row, &step)).sum();
            let pred = -dot(&gamma, &step) - 0.5 * quadratic;
            #[cfg(test)]
            if matches!(kind, TrialKind::Model | TrialKind::Mirror) && pred < 0.0 {
                super::work::add(|w| w.negative_pred += 1);
            }
            let mut correction = ProjectionCorrection::default();
            let projected = project_pose(s, &mut x, scale, &branches, report.tol, i, &mut correction, Some(projection_qr));
            let dp = [x[i] - s.x[i], x[i + 1] - s.x[i + 1]];
            let sum_d = [d[0] + (target[0] - x[i]), d[1] + (target[1] - x[i + 1])];
            let ared = 0.5 * dot(&dp, &sum_d);
            let ratio = if projected && pred > 0.0 { ared / pred } else { f64::NAN };
            let delta_p = if correction.exact { 0.0 } else { correction.stored };
            let eta = coordinate_floor.max(distance * delta_p);
            let below_floor = pred <= 2.0 * eta;
            let mut checked = None;
            // Q23: the final arm's three-way judge on res: gain, miss or unmeasured.
            let res_j = (4.0 * f64::EPSILON * pin_inf + correction.stored) * red.r_norm;
            let mut judge_class = 'x';
            let accepted = if !projected {
                false // ACC_NOT_PROJECTED
            } else if !below_floor {
                ared >= 1e-4 * pred // ACC_RATIO
            } else if flat {
                let (trial_r, trial_j, _) = s.evaluate(&x, scale, &branches);
                #[cfg(test)]
                super::work::add(|w| w.trial_qr += 1);
                let trial_qr = DragQr::new(&trial_j, n);
                let trial_d = [target[0] - x[i], target[1] - x[i + 1]];
                let trial_gamma = norm(&drag_gradient(&trial_qr, i, trial_d));
                let model_gamma: Vec<_> = hessian.iter().zip(&gamma).map(|(row, gj)| gj + dot(row, &step)).collect();
                let model_fall = report.gamma - norm(&model_gamma);
                if report.gamma - trial_gamma >= 0.5 * model_fall && trial_gamma < best_gamma_accept {
                    checked = Some((trial_r, trial_j, trial_qr, Some(trial_gamma)));
                    true
                } else {
                    false
                }
            } else {
                judge_class = if ared > res_j && ared >= 1e-4 * pred { 'G' } else if ared + res_j < pred / 4.0 { 'M' } else { 'U' };
                judge_class == 'G'
            };
            let fold = fold_choice.as_ref().map(|c| (c.sign, c.convention));
            let mut sheet_rejected = false;
            if accepted || (kind != TrialKind::Model && projected) {
                let (tr, tj, tq, gamma_accept) = checked.unwrap_or_else(|| {
                    let (tr, tj, _) = s.evaluate(&x, scale, &branches);
                    #[cfg(test)]
                    super::work::add(|w| w.trial_qr += 1);
                    let tq = DragQr::new(&tj, n);
                    (tr, tj, tq, None)
                });
                let tau = frame.trial_tangent(&tq, [x[i], x[i+1]], report.tol);
                let reading = frame.read(&tq, tau);
                let mut candidate = (x, tr, tj, tq, gamma_accept, reading, radius, boundary, ratio, tau);
                let chord = SheetChord::new(frame, reading, &s.x, &candidate.0);
                if ray.is_some() {
                    if chord.after > 0.0 && !accepted {
                        radius = SHRINK * length;
                        ray = None;
                        refused = None;
                        kind = TrialKind::Model;
                        other_side = false;
                        first_side = None;
                        if radius < f64::EPSILON * scale {
                            return false;
                        }
                        continue;
                    }
                }
                if regular && accepted && chord.after <= 0.0 {
                    #[cfg(test)]
                    super::work::add(|w| w.band_crossings += 1);
                    let past_term = chord.before > 0.0 && chord.past.is_some_and(|e| e <= report.tol);
                    let cmp_term = chord.before <= 0.0 && chord.after.abs() <= chord.before.abs();
                    let band = past_term || cmp_term || {
                        let nu1 = candidate.3.noise_floor(&candidate.2, 1.0).max(8.0 * f64::EPSILON);
                        let k = candidate.3.z.len();
                        // Every column of the sheet matrix is Zᵀw with Z orthonormal and w a unit vector, so by
                        // Hadamard each minor is at most 1 and the cofactor sum at most k²; 2 covers its rounding.
                        chord.after.abs() <= 2.0 * (k * k) as f64 * nu1 && {
                            let d_eps = nu1 * frame.sheet_scale(&candidate.3, tau);
                            chord.after.abs() <= d_eps
                        }
                    };
                    if !band {
                        sheet_rejected = true;
                    }
                }
                if kind == TrialKind::Twin && chord.after <= 0.0 && accepted && !sheet_rejected {
                    sheet_rejected = true;
                }
                if !accepted || sheet_rejected {
                    match kind {
                        TrialKind::Model => {
                            let mut source = Refusal { step, chord, twin: None };
                            if !accepted { round_clean = false; }
                            if chord.alpha < 0.5 {
                                kind = TrialKind::Mirror;
                            } else if frame.sheet_dim()>=2 {
                                kind = match make_twin(s, qr, &candidate.0, candidate.3,
                                    frame, chord, scale, report.tol) {
                                    TwinAttempt::Ready(twin) => {
                                        source.twin = Some(twin);
                                        TrialKind::Twin
                                    }
                                    TwinAttempt::Rounding | TwinAttempt::Overlength | TwinAttempt::ProbeFailed => TrialKind::Ray,
                                };
                            } else {
                                kind = TrialKind::Ray;
                            }
                            refused = Some(source);
                        }
                        TrialKind::Mirror | TrialKind::Twin => {
                            if kind==TrialKind::Mirror && !accepted { round_clean = false; }
                            if kind==TrialKind::Mirror && fold_start {
                                fold_shrink!();
                            }
                            kind=TrialKind::Ray;
                        }
                        TrialKind::Ray => {
                            ray.as_mut().unwrap().advance(chord.after);
                        }
                    }
                    if kind==TrialKind::Ray && ray.is_none() {
                        let initial = refused.as_ref().unwrap().chord;
                        let cut=RayCut::new(initial);
                        ray=Some(cut);
                    }
                    continue;
                }
                if let Some((side, convention)) = fold {
                    let wrong = if reading.reach > frame.reach { side != convention } else {
                        reading.reach == frame.reach && reading.sigma != frame.press_sign
                    };
                    if !regular && wrong {
                        if !other_side {
                            first_len = length;
                            first_side = Some(candidate);
                            other_side = true;
                            continue;
                        }
                        if frame.immutable() {
                            // Q20 section 3: both sides refused in a frame with no spare direction.
                            // sigma_press stays; the radius shrinks and the pair is tried again.
                            first_side.take().unwrap();
                            fold_level += 1;
                            let next = SHRINK * first_len;
                            radius = next;
                            other_side = false;
                            first_side = None;
                            ray = None;
                            refused = None;
                            kind = TrialKind::Model;
                            if radius < f64::EPSILON * scale {
                                return false;
                            }
                            continue;
                        }
                        candidate = first_side.take().unwrap();
                        frame.press_sign = candidate.5.sigma;
                    }
                }
                let (x, tr, tj, tq, gamma_accept, reading, used_radius, boundary, ratio, tau) = candidate;
                if !x.iter().all(|v| v.is_finite()) {
                    s.x.clone_from(&start);
                    restored_start = true;
                    frame.clone_from(&start_frame);
                    report.outcome = DragOutcome::NotReached;
                    eprintln!("constrain drag: a trial pose is not finite; the move restores its start");
                    return false;
                }
                let chord = SheetChord::new(frame, reading, &s.x, &x);
                let restored = fold_start && fold_level > 0 && round_clean
                    && chord.after > 0.0 && reading.reach >= frame.reach;
                if reading.reach > frame.reach {
                    frame.tau_point = [x[i], x[i+1]];
                    frame.set_basis(&tq);
                } else {
                    if frame.reach == 1 && reading.reach == 1 {
                        frame.tau = tau;
                        frame.tau_point = [x[i], x[i+1]];
                    }
                    frame.current = reading;
                }
                if let Some(g) = gamma_accept { best_gamma_accept = g; }
                s.x = x;
                r = tr;
                jac = tj;
                cached_qr = Some(tq);
                radius = used_radius;
                if restored {
                    radius = round_radius;
                } else if boundary && ratio > 0.75 { radius *= 2.0; }
                break;
            }
            let dp_proj_here = correction.stored;
            round_clean = false;
            if kind != TrialKind::Model {
                let source=refused.as_ref().unwrap();
                if kind==TrialKind::Ray {
                    eprintln!("constrain drag: a ray-cut trial did not project; the move keeps its last accepted pose");
                    return false;
                }
                if kind==TrialKind::Mirror && fold_start {
                    fold_shrink!();
                }
                let cut=RayCut::new(source.chord);
                ray=Some(cut);
                kind=TrialKind::Ray;
                continue;
            }
            let dp_proj_a = dp_proj_here;
            let res_a = (4.0 * f64::EPSILON * pin_inf + dp_proj_a) * red.r_norm;
            let below_res = if judge_class != 'x' { judge_class == 'U' && projected && original_length > 0.0 } else { projected && pred <= res_a && ared.abs() <= res_a && original_length > 0.0 };
            if below_res && (!boundary || pred == 0.0) {
                // R1: shrinking here would oscillate with the grow.
                report.outcome = DragOutcome::NotReached;
                return false;
            }
            let unmeasured = below_res && boundary;
            if unmeasured && original_length / SHRINK >= RUNAWAY * scale {
                s.x.clone_from(&start);
                restored_start = true;
                frame.clone_from(&start_frame);
                report.outcome = DragOutcome::NotReached;
                eprintln!("constrain drag: a trial radius ran away (R2); the move restores its start");
                return false;
            }
            if unmeasured {
                r_u = Some(r_u.map_or(radius, |v: f64| v.max(radius)));
                radius = original_length / SHRINK;
                continue;
            }
            if let Some(ru) = r_u {
                if SHRINK * original_length <= ru * (1.0 + 1e-9) {
                    // R9: a refusal after rule (a) grew the radius may not shrink it back to r_u or below.
                    report.outcome = DragOutcome::NotReached;
                    return false;
                }
            }
            radius = SHRINK * original_length;
            if radius < f64::EPSILON * scale {
                return false;
            }
        }
        true
    };
    while advance_one_round() {}
    if !restored_start && report.rounds > 0 {
        *kept = cached_qr.map(|qr| Kept { pose: s.x.clone(), scale, r, jac, qr });
    }
    MoveEnd { rounds: report.rounds, outcome: report.outcome }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symmetric(k: usize, mut entry: impl FnMut(usize, usize) -> f64) -> Vec<Vec<f64>> {
        let mut matrix = vec![vec![0.0; k]; k];
        for i in 0..k {
            for j in 0..=i {
                let value = entry(i, j);
                matrix[i][j] = value;
                matrix[j][i] = value;
            }
        }
        matrix
    }

    fn same_bits(a: &[f64], b: &[f64]) -> bool {
        a.iter().map(|v| v.to_bits()).eq(b.iter().map(|v| v.to_bits()))
    }

    #[test]
    fn small_eigen_ql_against_jacobi() {
        let mut state = 0x6b27_f335_856c_d419u64;
        let mut sample = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 * (2.0 / (1u64 << 53) as f64) - 1.0
        };
        let mut ql_ops_32 = 0;
        for k in [1, 2, 5, 16, 17, 32, 64] {
            let random = symmetric(k, |_, _| sample());
            let b0: Vec<_> = (0..k).map(|_| sample()).collect();
            let b1: Vec<_> = (0..k).map(|_| sample()).collect();
            let gram = symmetric(k, |i, j| b0[i] * b0[j] + b1[i] * b1[j]);
            let perturbed = symmetric(k, |i, j| gram[i][j] + 1e-6 * random[i][j]);

            let mut q = vec![vec![0.0; k]; k];
            for i in 0..k { q[i][i] = 1.0; }
            for _ in 0..3 {
                let u: Vec<_> = (0..k).map(|_| sample()).collect();
                let beta = 2.0 / dot(&u, &u);
                for row in &mut q {
                    let weight = beta * dot(row, &u);
                    for i in 0..k { row[i] -= weight * u[i]; }
                }
            }
            let repeated = symmetric(k, |i, j| {
                (0..k).map(|l| q[i][l] * (l / 3) as f64 * q[j][l]).sum()
            });

            for (kind, matrix) in [("random", random), ("Gram", gram),
                                   ("Gram plus 1e-6", perturbed), ("repeated", repeated)] {
                work::take();
                let jacobi = SmallEigen::jacobi(matrix.clone());
                let jacobi_ops = work::take().eigen_jacobi_ops;
                let ql = SmallEigen::ql(matrix.clone());
                let ql_ops = work::take().eigen_ql_ops;
                let magnitude = jacobi.values[0].abs().max(jacobi.values[k - 1].abs());
                let bound = 64.0 * k as f64 * f64::EPSILON * magnitude;
                assert!(ql.values.windows(2).all(|pair| pair[0] <= pair[1]),
                    "{kind} k={k} QL eigenvalues must be ascending");
                for (j, v) in ql.vectors.iter().enumerate() {
                    let error = (ql.values[j] - jacobi.values[j]).abs();
                    assert!(error <= bound, "{kind} k={k} eigenvalue {j} differs by {error:e}, bound {bound:e}");
                    for (i, u) in ql.vectors[..=j].iter().enumerate() {
                        let target = if i == j { 1.0 } else { 0.0 };
                        let error = (dot(u, v) - target).abs();
                        assert!(error <= 1e-12, "{kind} k={k} orthonormality error at ({i},{j}) is {error:e}");
                    }
                    let residual = matrix.iter().zip(v).map(|(row, &x)| {
                        (dot(row, v) - ql.values[j] * x).powi(2)
                    }).sum::<f64>().sqrt();
                    assert!(residual <= bound, "{kind} k={k} eigenvector {j} residual is {residual:e}, bound {bound:e}");
                }

                if k == 16 || k == 17 {
                    let auto = if k == 16 { &jacobi } else { &ql };
                    for (mode, expected) in [(EigenSolver::Auto, auto), (EigenSolver::Jacobi, &jacobi), (EigenSolver::Ql, &ql)] {
                        let previous = set_eigen_solver(mode);
                        let actual = SmallEigen::new(matrix.clone());
                        set_eigen_solver(previous);
                        assert!(same_bits(&actual.values, &expected.values),
                            "{kind} k={k} {mode:?} dispatched eigenvalue bits differ");
                        for j in 0..k {
                            assert!(same_bits(&actual.vectors[j], &expected.vectors[j]),
                                "{kind} k={k} {mode:?} dispatched eigenvector {j} bits differ");
                        }
                    }
                }
                if kind == "random" && k == 32 { ql_ops_32 = ql_ops; }
                if kind == "random" && k == 64 {
                    assert!(ql_ops < 12 * ql_ops_32,
                        "{kind} k={k} QL work {ql_ops} must be below 12 times its k=32 work {ql_ops_32}");
                    assert!(ql_ops < jacobi_ops,
                        "{kind} k={k} QL work {ql_ops} must be below Jacobi work {jacobi_ops}");
                }
            }
        }
    }

    #[test]
    fn ql_drag_matches_jacobi_on_a_coupled_chain() {
        use super::super::Constraint;

        let opts = Options::default();
        // Built in M3b's order (points, lines, the fix, then the lengths), so its measured spread applies.
        let mut base = System::new();
        let mut points = vec![base.point([0.0, 0.0])];
        let mut at = [0.0_f64, 0.0];
        for i in 0..25 {
            let angle = (i as f64 * (90.0 / 25.0)).to_radians();
            at = [at[0] + 4.0 * angle.cos(), at[1] + 4.0 * angle.sin()];
            points.push(base.point(at));
        }
        let lines: Vec<_> = points.windows(2).map(|pair| base.line(pair[0], pair[1])).collect();
        base.constrain(Constraint::Fix(points[0], [0.0, 0.0]));
        for line in lines {
            base.constrain(Constraint::Length(line, 4.0));
        }
        let end = points[25];
        assert!(base.solve(&opts).converged, "the 25-link chain must converge at the press before the three targets");

        let at = base.at(end);
        let tol = opts.tolerance * base.scale();
        let run = |solver, target| {
            let mut s = base.clone();
            let previous = set_eigen_solver(solver);
            let mut drag = s.start_drag(end, &opts);
            let report = drag.to(&mut s, target);
            set_eigen_solver(previous);
            (s, report)
        };
        for k in 1..=3 {
            let target = [at[0] - 0.25 * k as f64, at[1] - 0.15 * k as f64];
            let (reference, jacobi) = run(EigenSolver::Jacobi, target);
            let (actual, auto) = run(EigenSolver::Auto, target);
            assert_eq!(auto.outcome, jacobi.outcome, "target {k} {target:?} must give the same outcome");
            assert_eq!(auto.rounds, jacobi.rounds, "target {k} {target:?} must take the same number of rounds");
            assert_eq!(auto.side_enforced, jacobi.side_enforced, "target {k} {target:?} must give the same side enforcement");
            for (solver, residual) in [("Jacobi", jacobi.residual), ("Auto", auto.residual)] {
                assert!(residual <= tol, "target {k} {target:?} {solver} residual {residual:e} exceeds {tol:e}");
            }
            let reference_pin = reference.at(end);
            let actual_pin = actual.at(end);
            for axis in 0..2 {
                let error = (actual_pin[axis] - reference_pin[axis]).abs();
                assert!(error <= tol / 100.0, "target {k} {target:?} pin coordinate {axis} differs by {error:e}, above tol/100");
            }
            // M3b (Q28): on these three moves the exact controls (Jacobi on the index-reversed matrix,
            // and on H scaled by 1 ± ε by the parity of i + j) spread the pose by S <= 0.14 tol.
            // The bound is 2 tol, the next round number above 8 S = 1.11 tol. QL measured 0.05 to 0.13 tol.
            let pose_bound = 2.0 * tol;
            for i in 0..base.x.len() {
                let error = (actual.x[i] - reference.x[i]).abs();
                assert!(error <= pose_bound, "target {k} {target:?} pose coordinate {i} differs by {error:e}, above {pose_bound:e}");
            }
        }
    }
}
