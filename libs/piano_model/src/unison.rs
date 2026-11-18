//! The strings of one note as one coupled system (Weinreich, JASA 1977).
//!
//! Every string of a unison ends on the same point of the bridge, in both
//! of its polarisations. Per partial, in the frame rotating at the partial's
//! frequency, the complex envelopes a = (vertical 1..N, horizontal 1..N)
//! follow
//!
//!     da/dt = A a,   A[(p,i),(q,j)] = delta d_(p,i) - kappa y_pq
//!     d_(v,i) = -s + j e_i              d_(h,i) = -s hl + j (e_i + e_pol)
//!     kappa   = (g + j pull) / N        y = [[1, chi], [chi, eta]]
//!
//! `s` is the string's own loss, `e_i` string i's mistuning (rad/s), `g` the
//! rate at which the bridge drains the strings moving together, `pull` its
//! reactive part and `y` the bridge's normalised admittance over the two
//! directions. The coupling term is the same for every pair of strings:
//! that is all the unison physics there is. Prompt and aftersound, the beat
//! and the lock are what this system's modes do, and none of them is put in
//! by hand:
//!
//! - strings closer together than the drain lock: one mode takes nearly the
//!   whole blow and falls at s + g, and what is left rings on at about s;
//! - strings further apart ring one mode each at s + g / N and beat;
//! - a blow that is uneven across the strings leaves an aftersound in
//!   quadrature with the prompt, which is why a well-tuned unison's
//!   aftersound has no hole where the two meet.
//!
//! The hammer sets a(0) = u, with u_(v,i) = h_i and u_(h,i) = pd h_i. The
//! bridge force is read as w_main = (1/N .. | 0 ..) vertically and
//! w_side = (0 .. | rho/N ..) horizontally, and for the modes
//! A x_m = lambda_m x_m the heard partial is sum_m R_m exp(lambda_m t) with
//! R_m = (w . x_m)(x_m . u) / (x_m . x_m), no conjugates: A is complex
//! symmetric.
//!
//! `solve` runs once per partial at construction; nothing here runs per
//! sample. The system is at most 6 x 6, so its modes come from the
//! characteristic polynomial: Faddeev-LeVerrier gives the polynomial and,
//! with it, the adjugate of z - A as a matrix polynomial, and the adjugate
//! at a root over the polynomial's derivative there is that mode's
//! projector x x^T / (x . x). No eigenvector is formed, and two modes at
//! the same frequency (strings in perfect tune) need no special case.

use crate::keys::C64;

pub const MAX_STRINGS: usize = 3;
pub const MAX_MODES: usize = 2 * MAX_STRINGS;

/// One partial of one note: see the module notes for the symbols.
#[derive(Clone, Copy, Debug)]
pub struct Unison {
    pub strings: usize,
    /// Intrinsic loss of the vertical polarisation, 1/s.
    pub s: f64,
    /// The horizontal polarisation's loss as a multiple of `s`.
    pub hl: f64,
    /// Each string's mistuning at this partial, rad/s.
    pub e: [f64; MAX_STRINGS],
    /// Offset of the horizontal polarisation, rad/s.
    pub e_pol: f64,
    /// In-phase drain of the whole unison, 1/s, and its reactive part.
    pub g: f64,
    pub pull: f64,
    /// Bridge cross term and horizontal share of the admittance.
    pub chi: f64,
    pub eta: f64,
    /// The hammer's sideways drive and how the horizontal force radiates.
    pub pd: f64,
    pub rho: f64,
    /// The blow on each string, mean 1.
    pub h: [f64; MAX_STRINGS],
}

/// One mode of the coupled system.
#[derive(Clone, Copy, Debug)]
pub struct Mode {
    /// Rate and frequency offset: exp(lambda t), 1/s and rad/s.
    pub lambda: C64,
    /// Residue in the vertical bridge force.
    pub main: C64,
    /// Residue in the horizontal bridge force.
    pub side: C64,
    /// Share of the mode's motion that is vertical (1 for a purely vertical
    /// mode, 0 for a horizontal one).
    pub vert: f64,
}

const ZERO: C64 = C64 { re: 0.0, im: 0.0 };
const ONE: C64 = C64 { re: 1.0, im: 0.0 };
type Matrix = [[C64; MAX_MODES]; MAX_MODES];

/// Where the modes of an even pair meet (mistuning = drain) the system has
/// one mode less than its size: the two residues grow without bound, with
/// opposite signs, and single-precision oscillators cannot cancel them. A
/// residue past this many times the whole response means the partial sits
/// on that point; its strings are moved apart by 3 % and solved again.
const RESIDUE_BOUND: f64 = 8.0;

/// The modes of one partial, in no particular order, and how many there are
/// (twice the string count).
pub fn solve(unison: &Unison) -> ([Mode; MAX_MODES], usize) {
    let mut u = *unison;
    let n = u.strings.clamp(1, MAX_STRINGS);
    let whole = u.h[..n].iter().sum::<f64>().abs() / n as f64;
    let mut out = decompose(&u);
    for _ in 0..4 {
        let bound = RESIDUE_BOUND * whole.max(1e-12);
        if out.0[..out.1].iter().all(|m| m.main.abs2().sqrt() <= bound && m.side.abs2().sqrt() <= bound * u.rho.abs().max(1e-12)) {
            break;
        }
        u.e.iter_mut().for_each(|e| *e *= 1.03);
        out = decompose(&u);
    }
    out
}

fn decompose(u: &Unison) -> ([Mode; MAX_MODES], usize) {
    let ns = u.strings.clamp(1, MAX_STRINGS);
    let n = 2 * ns;
    let kappa = C64::new(u.g / ns as f64, u.pull / ns as f64);
    let mut a: Matrix = [[ZERO; MAX_MODES]; MAX_MODES];
    for i in 0..n {
        for j in 0..n {
            let y = match (i < ns, j < ns) {
                (true, true) => 1.0,
                (false, false) => u.eta,
                _ => u.chi,
            };
            a[i][j] = kappa.scale(-y);
        }
        let own = if i < ns { C64::new(-u.s, u.e[i]) } else { C64::new(-u.s * u.hl, u.e[i - ns] + u.e_pol) };
        a[i][i] = a[i][i].add(own);
    }
    // Centre and scale: the roots then lie within a few units of zero
    // whatever the partial's rates are.
    let mut centre = ZERO;
    for i in 0..n {
        centre = centre.add(a[i][i]);
    }
    centre = centre.scale(1.0 / n as f64);
    let mut size = 0.0f64;
    for i in 0..n {
        a[i][i] = a[i][i].sub(centre);
        for j in 0..n {
            size = size.max(a[i][j].abs2());
        }
    }
    let size = size.sqrt().max(1e-300);
    for row in a.iter_mut().take(n) {
        for v in row.iter_mut().take(n) {
            *v = v.scale(1.0 / size);
        }
    }
    // Faddeev-LeVerrier: det(z - A) = z^n + c[1] z^(n-1) + .. + c[n] and
    // adj(z - A) = sum_k z^(n-1-k) m[k].
    let mut m = [[[ZERO; MAX_MODES]; MAX_MODES]; MAX_MODES];
    let mut c = [ZERO; MAX_MODES + 1];
    c[0] = ONE;
    for i in 0..n {
        m[0][i][i] = ONE;
    }
    for k in 1..=n {
        let mut am: Matrix = [[ZERO; MAX_MODES]; MAX_MODES];
        let mut trace = ZERO;
        for i in 0..n {
            for j in 0..n {
                let mut sum = ZERO;
                for l in 0..n {
                    sum = sum.add(a[i][l].mul(m[k - 1][l][j]));
                }
                am[i][j] = sum;
            }
            trace = trace.add(am[i][i]);
        }
        c[k] = trace.scale(-1.0 / k as f64);
        if k < n {
            for i in 0..n {
                am[i][i] = am[i][i].add(c[k]);
            }
            m[k] = am;
        }
    }
    // value and derivative of the polynomial at z
    let poly = |z: C64| {
        let (mut p, mut d) = (ONE, ZERO);
        for ck in &c[1..=n] {
            d = d.mul(z).add(p);
            p = p.mul(z).add(*ck);
        }
        (p, d)
    };
    // Aberth: all roots at once, each step a Newton step that also keeps
    // the roots off one another.
    let mut z = [ZERO; MAX_MODES];
    for (k, zk) in z.iter_mut().enumerate().take(n) {
        *zk = C64::expj(core::f64::consts::TAU * k as f64 / n as f64 + 0.4).scale(0.6 + 0.07 * k as f64);
    }
    for _ in 0..200 {
        let mut moved = 0.0f64;
        for k in 0..n {
            let (p, d) = poly(z[k]);
            if p.abs2() == 0.0 {
                continue;
            }
            if d.abs2() == 0.0 {
                // on a stationary point of the polynomial: step off it
                z[k] = z[k].add(C64::new(1e-6, 1e-6));
                moved = 1.0;
                continue;
            }
            let w = p.div(d);
            let mut repel = ZERO;
            for j in (0..n).filter(|&j| j != k) {
                let gap = z[k].sub(z[j]);
                if gap.abs2() > 0.0 {
                    repel = repel.add(ONE.div(gap));
                }
            }
            let den = ONE.sub(w.mul(repel));
            let step = if den.abs2() > 1e-24 { w.div(den) } else { w };
            z[k] = z[k].sub(step);
            moved = moved.max(step.abs2());
        }
        if moved < 1e-30 {
            break;
        }
    }
    // Two roots on the very same value would divide by a zero derivative;
    // a hair apart they share the pair's projector between them.
    for k in 1..n {
        for j in 0..k {
            if z[k].sub(z[j]).abs2() < 1e-24 {
                z[k] = z[k].add(C64::new(5e-10, 0.0));
                z[j] = z[j].sub(C64::new(5e-10, 0.0));
            }
        }
    }
    let mut out = [Mode { lambda: ZERO, main: ZERO, side: ZERO, vert: 0.0 }; MAX_MODES];
    for (k, mode) in out.iter_mut().enumerate().take(n) {
        let (_, d) = poly(z[k]);
        mode.lambda = z[k].scale(size).add(centre);
        if d.abs2() == 0.0 {
            continue;
        }
        // (adj u)_i for the main and side sums, and the adjugate's
        // vertical trace
        let (mut main, mut side, mut vert) = (ZERO, ZERO, ZERO);
        for i in 0..n {
            let mut row = ZERO;
            let mut diag = ZERO;
            for j in 0..n {
                let mut adj = ZERO;
                for mk in m.iter().take(n) {
                    adj = adj.mul(z[k]).add(mk[i][j]);
                }
                let blow = if j < ns { u.h[j] } else { u.pd * u.h[j - ns] };
                row = row.add(adj.scale(blow));
                if i == j {
                    diag = adj;
                }
            }
            if i < ns {
                main = main.add(row);
                vert = vert.add(diag);
            } else {
                side = side.add(row);
            }
        }
        mode.main = main.div(d).scale(1.0 / ns as f64);
        mode.side = side.div(d).scale(u.rho / ns as f64);
        mode.vert = vert.div(d).re;
    }
    (out, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A middle trichord's partial with both polarisations in play.
    fn partial() -> Unison {
        Unison {
            strings: 3,
            s: 0.9,
            hl: 0.5,
            e: [0.31, -0.07, -0.24],
            e_pol: 1.1,
            g: 1.7,
            pull: 0.4,
            chi: 0.035,
            eta: 0.02,
            pd: 0.3,
            rho: 0.55,
            h: [1.15, 0.95, 0.90],
        }
    }

    /// The same strings moving vertically only.
    fn vertical(strings: usize, e: [f64; 3], h: [f64; 3]) -> Unison {
        Unison { strings, e, h, pull: 0.0, chi: 0.0, eta: 0.0, pd: 0.0, rho: 0.0, ..partial() }
    }

    fn heard(modes: &[Mode], t: f64) -> (C64, C64) {
        let (mut main, mut side) = (ZERO, ZERO);
        for m in modes {
            let turn = C64::expj(m.lambda.im * t).scale((m.lambda.re * t).exp());
            main = main.add(m.main.mul(turn));
            side = side.add(m.side.mul(turn));
        }
        (main, side)
    }

    fn close(a: C64, b: C64, tol: f64, what: &str) {
        assert!(a.sub(b).abs2().sqrt() <= tol, "{what}: {a:?} against {b:?}");
    }

    #[test]
    fn modes_carry_the_whole_blow_and_follow_the_equations() {
        let u = partial();
        let (modes, n) = solve(&u);
        let modes = &modes[..n];
        assert_eq!(n, 6);
        // completeness: at the strike the modes sum to the blow as read
        let (main, side) = heard(modes, 0.0);
        close(main, C64::new(1.0, 0.0), 1e-9, "main residues sum to w_main . u");
        close(side, C64::new(u.rho * u.pd, 0.0), 1e-9, "side residues sum to w_side . u");
        // passivity: no mode outlasts the slowest string loss
        for m in modes {
            assert!(m.lambda.re <= -u.s * u.hl + 1e-9, "a mode decays slower than the strings: {m:?}");
        }
        let vert: f64 = modes.iter().map(|m| m.vert).sum();
        assert!((vert - 3.0).abs() < 1e-9, "the vertical shares sum to the string count: {vert}");
        // the modal sum is the solution of da/dt = A a (stepped, RK4)
        let ns = u.strings;
        let kappa = C64::new(u.g / ns as f64, u.pull / ns as f64);
        let deriv = |a: &[C64; 6]| {
            let mut out = [ZERO; 6];
            let (mut sv, mut sh) = (ZERO, ZERO);
            for i in 0..ns {
                sv = sv.add(a[i]);
                sh = sh.add(a[ns + i]);
            }
            for i in 0..ns {
                out[i] = C64::new(-u.s, u.e[i]).mul(a[i]).sub(kappa.mul(sv.add(sh.scale(u.chi))));
                out[ns + i] =
                    C64::new(-u.s * u.hl, u.e[i] + u.e_pol).mul(a[ns + i]).sub(kappa.mul(sv.scale(u.chi).add(sh.scale(u.eta))));
            }
            out
        };
        let mut a = [ZERO; 6];
        for i in 0..ns {
            a[i] = C64::new(u.h[i], 0.0);
            a[ns + i] = C64::new(u.pd * u.h[i], 0.0);
        }
        let dt = 1e-3;
        let axpy = |a: &[C64; 6], k: &[C64; 6], f: f64| -> [C64; 6] { core::array::from_fn(|i| a[i].add(k[i].scale(f))) };
        for step in 1..=1500 {
            let k1 = deriv(&a);
            let k2 = deriv(&axpy(&a, &k1, 0.5 * dt));
            let k3 = deriv(&axpy(&a, &k2, 0.5 * dt));
            let k4 = deriv(&axpy(&a, &k3, dt));
            for i in 0..6 {
                a[i] = a[i].add(k1[i].add(k2[i].scale(2.0)).add(k3[i].scale(2.0)).add(k4[i]).scale(dt / 6.0));
            }
            if step == 500 || step == 1500 {
                let (mut main, mut side) = (ZERO, ZERO);
                for i in 0..ns {
                    main = main.add(a[i].scale(1.0 / ns as f64));
                    side = side.add(a[ns + i].scale(u.rho / ns as f64));
                }
                let (hm, hs) = heard(modes, step as f64 * dt);
                close(hm, main, 1e-6, "main at a stepped time");
                close(hs, side, 1e-6, "side at a stepped time");
            }
        }
    }

    #[test]
    fn tuning_decides_between_one_locked_mode_and_one_mode_per_string() {
        // perfect tune: the in-phase mode has the whole blow and the whole
        // drain, even when the blow is uneven. (The other modes then share
        // one frequency two and three at a time, and a repeated root of the
        // polynomial is found to about the cube root of the arithmetic's
        // precision: the bound is -100 dB, not exactly nothing.)
        for strings in [2usize, 3] {
            let u = vertical(strings, [0.0; 3], [1.2, 0.9, if strings == 3 { 0.9 } else { 0.0 }]);
            let mean = u.h[..strings].iter().sum::<f64>() / strings as f64;
            let (modes, n) = solve(&u);
            let mut whole = ZERO;
            for m in &modes[..n] {
                if (m.lambda.re + u.s + u.g).abs() < 1e-6 {
                    whole = whole.add(m.main);
                } else {
                    assert!(m.main.abs2().sqrt() < 1e-5, "{strings} strings in tune: a second sounding mode {m:?}");
                }
            }
            close(whole, C64::new(mean, 0.0), 1e-5, "in tune: the mode at s + g carries the blow");
        }
        // far apart: every string its own mode, 1/N of the blow, at s + g/N
        let u = vertical(3, [400.0, -100.0, -300.0], [1.0; 3]);
        let (modes, n) = solve(&u);
        let mut sounding = 0;
        for m in modes[..n].iter().filter(|m| m.vert > 0.5) {
            sounding += 1;
            assert!((m.lambda.re + u.s + u.g / 3.0).abs() < 0.02, "apart: rate {m:?}");
            assert!((m.main.abs2().sqrt() - 1.0 / 3.0).abs() < 0.01, "apart: residue {m:?}");
            assert!(u.e.iter().any(|e| (m.lambda.im - e).abs() < 0.02), "apart: each mode on a string's own frequency {m:?}");
        }
        assert_eq!(sounding, 3);
        // two strings at +-e, blow (1 + d, 1 - d): lambda = -s - k +- r with
        // r^2 = k^2 - e^2 and residue (-e^2 + j e d mu) / (mu^2 - e^2),
        // mu = k +- r, on both sides of the lock (e = k)
        let d = 0.15;
        for e in [0.3, 2.0] {
            let u = vertical(2, [e, -e, 0.0], [1.0 + d, 1.0 - d, 0.0]);
            let k = 0.5 * u.g;
            let r = if e < k { C64::new((k * k - e * e).sqrt(), 0.0) } else { C64::new(0.0, (e * e - k * k).sqrt()) };
            let (modes, n) = solve(&u);
            for sign in [1.0, -1.0] {
                let lambda = C64::new(-u.s - k, 0.0).add(r.scale(sign));
                let mu = C64::new(k, 0.0).add(r.scale(sign));
                let residue = C64::new(-e * e, 0.0).add(mu.mul(C64::new(0.0, e * d))).div(mu.mul(mu).sub(C64::new(e * e, 0.0)));
                let m = modes[..n].iter().filter(|m| m.vert > 0.5).min_by(|a, b| a.lambda.sub(lambda).abs2().total_cmp(&b.lambda.sub(lambda).abs2())).unwrap();
                close(m.lambda, lambda, 1e-9, "two strings: the closed form's rate and frequency");
                close(m.main, residue, 1e-9, "two strings: the closed form's residue");
            }
        }
        // the lock itself: an even pair at e = k has one mode less than its
        // size; the solve answers with bounded residues that still sum to 1
        let u = vertical(2, [0.85, -0.85, 0.0], [1.0, 1.0, 0.0]);
        let (modes, n) = solve(&u);
        let mut sum = ZERO;
        for m in &modes[..n] {
            assert!(m.main.abs2().sqrt() <= RESIDUE_BOUND, "at the lock: residue {m:?}");
            sum = sum.add(m.main);
        }
        close(sum, C64::new(1.0, 0.0), 1e-9, "at the lock: completeness");
    }

    #[test]
    fn one_string_is_the_polarisation_pair() {
        // The two polarisations of a single string as keys::build_key
        // reduces them today. That reduction leaves the bridge's pull out of
        // the horizontal polarisation's own term, so e_pol carries it here.
        let (s, hl, g, pull, couple, cross, pd, rho, dw) = (2.3, 0.5, 6.1, -1.9, 0.02, 0.25, 0.27, 0.55, 0.8);
        let d_v = C64::new(-s - g, -pull);
        let d_h = C64::new(-s * hl - g * couple, dw);
        let pair = crate::keys::polarisation_pair(d_v, d_h, C64::new(g, pull), couple, cross, pd, rho);
        let u = Unison {
            strings: 1,
            s,
            hl,
            e: [0.0; 3],
            e_pol: dw + pull * couple,
            g,
            pull,
            chi: cross * couple.sqrt(),
            eta: couple,
            pd,
            rho,
            h: [1.0, 0.0, 0.0],
        };
        let (modes, n) = solve(&u);
        assert_eq!(n, 2);
        for (lambda, residue) in pair {
            let m = modes[..n].iter().min_by(|a, b| a.lambda.sub(lambda).abs2().total_cmp(&b.lambda.sub(lambda).abs2())).unwrap();
            close(m.lambda, lambda, 1e-6, "one string: rate and frequency");
            close(m.main.add(m.side), residue, 1e-6, "one string: residue");
        }
    }
}
