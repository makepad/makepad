//! The town (the page's `pkTown`): four rows of houses, back to front, on the shared site (level along a quay, climbing a
//! hillside inland), each row at its own distance with its own band of ground; roofs by the town's style, lit windows
//! by rooms, shops, attic and dormer windows; street lamps in front of each row, quay lamps, washing the walls behind.
use super::kit::{clamp, town_td, Ctx, Pt::*, I0, Instance};
use crate::params::P;
use crate::rng::{fract, mix, smooth, wd, TAU};

/// A street lamp: m along the row, azimuth, its ground, on the quay, its colour.
#[derive(Clone, Copy)]
struct Lamp { xl: f64, ul: f64, gl: f64, quay: bool, kw: f64 }
/// A house of the row: one block over two cells, its id, half-width, centre, nearness to the centre, waterfront share, lamps.
struct House { blk: bool, hid: f64, hw: f64, xh: f64, cn: f64, wf: f64, lamps: Vec<Lamp> }

pub fn build(c: &mut Ctx) {
    let (z, zs) = (c.z.town, c.z);
    if c.p[P::PrTown] <= 0.0 || z[1] <= 0.0 { return; }
    let rnd = |id: f64, st: f64, ly: f64| zs.rnd(id, st, ly);
    let (s, sty, sea, shore) = (c.town_site(), rnd(5.0, 1.0, 1600.0), zs.town_x[1], zs.town_x[0]);
    let (wp, sod, dens) = (c.p[P::WinD] * mix(0.5, 0.9, rnd(6.0, 1.0, 1600.0)), if rnd(7.0, 1.0, 1600.0) < 0.65 { 0.75 } else { 1.6 }, 0.5 + 0.5 * c.p[P::PrTown]);
    let inland = |u: f64| if sea != 0.0 { smooth(0.0, 0.012, -wd(u, shore) * sea) } else { 1.0 }; // 0 on the waterfront .. 1 inland
    let lamp = |c: &mut Ctx, l: &Lamp, x: f64| {
        let hl = if l.quay { 4.3 } else { 5.5 };
        c.trap(x, l.gl, 0.06, 0.06, hl, 1.0); c.light(x, l.gl + hl, if l.quay { 0.16 } else { 0.2 }, if l.quay { 1.8 } else { 1.3 }, l.kw);
    };
    for k in (0..4).rev() {
        let kf = k as f64;
        // the row d m out; its houses, lamps and ground in size units, which subtend over dk = d / Size what metres do over d
        let (d, ly) = (s[0] + kf * s[1], kf * 8.0);
        let dk = d / c.k;
        let (xx, xu) = (z[1] * TAU * dk, |x: f64| z[0] + x / (TAU * dk));
        // the row's band of ground: columns between samples every 1/1680 turn (Size times that), eight per instance
        let gn = 1680.0 / c.k;
        let gs: Vec<[f64; 2]> = (((z[0] - z[1]) * gn).floor() as i64..=((z[0] + z[1]) * gn).ceil() as i64)
            .map(|j| { let u = (j as f64 / gn).max(z[0] - z[1]).min(z[0] + z[1]); [(u - z[0]) * TAU * dk, c.town_ground(u, kf, &s)] }).collect();
        for seg in (0..gs.len().saturating_sub(1)).step_by(8).map(|j| &gs[j..(j + 9).min(gs.len())]).filter(|q| q.iter().any(|q| q[1] >= 0.02)) {
            let xc = 0.5 * (seg[0][0] + seg[seg.len() - 1][0]);
            c.begin(Instance { uc: fract(xu(xc)), d, td: town_td(d, 0.9), ..I0 }, 0.0, 0.0);
            for w in seg.windows(2).filter(|w| w[1][0] > w[0][0] + 1e-3) { c.part(GProf, [w[0][0] - xc, w[0][1], w[1][0] - xc, w[1][1]], [0., 0., 1., 0.]); }
            c.end(0.5, 0.5);
        }
        // the street in front of this row: its lamps (a continuous string along the quay)
        let cc = mix(9.0, 13.5, rnd(kf, 1.0, 1610.0));
        let (off, ls) = (rnd(kf, 2.0, 1610.0) * 2.0 * cc, if k == 0 { 17.0 } else { mix(24.0, 34.0, rnd(kf, 3.0, 1610.0)) });
        let (offl, mut lamps) = (rnd(kf, 4.0, 1610.0) * ls, vec![]);
        let mut jl = ((offl - 0.94 * xx) / ls).ceil();
        while jl * ls - offl <= 0.94 * xx {
            let xl = jl * ls - offl;
            let (ul, cn) = (xu(xl), 1.0 - smooth(0.0, z[1].max(0.03), (xl / (TAU * dk)).abs()));
            let quay = k == 0 && inland(ul) <= 0.5;
            if quay || rnd(jl, 4.0 * kf + 20.0, 1612.0) <= 0.75 * mix(0.5, 1.0, cn) {
                lamps.push(Lamp { xl, ul, gl: c.town_ground(ul, kf - 0.12, &s), quay, kw: if quay || sod < 1.0 { 0.75 } else { sod } });
            }
            jl += 1.0;
        }
        // the houses: cells of cc m, pairs sometimes one larger block; whole houses inside the zone; on the quay terraced
        let mut hs: Vec<House> = vec![];
        for ci in ((off - xx) / cc).floor() as i64 - 1..=((off + xx) / cc).floor() as i64 + 1 {
            let (ce, cf) = ((ci - ci.rem_euclid(2)) as f64, ci as f64);
            let blk = rnd(ce, ly + 3.0, 1611.0) <= mix(0.3, 0.12, sty);
            if blk && cf != ce { continue; } // a block over a pair of cells: once
            let hid = if blk { ce } else { cf };
            let (rw, rj) = (rnd(hid, ly + 4.0, 1611.0), rnd(hid, ly + 5.0, 1611.0));
            let (hw, xh) = if blk { let hw = cc * mix(0.8, 0.98, rw); (hw, (ce + 1.0) * cc + (rj - 0.5) * (2.0 * cc - 2.0 * hw)) } else {
                let wfc = if k == 0 && sea != 0.0 { 1.0 - inland(xu((cf + 0.5) * cc - off)) } else { 0.0 };
                let hw = 0.5 * cc * mix(mix(0.55, 0.85, wfc), 1.0, rw * rw * (3.0 - 2.0 * rw));
                (hw, (cf + 0.5) * cc + (rj - 0.5) * (cc - 2.0 * hw))
            };
            let (xh, duh) = (xh - off, (xh - off) / (TAU * dk));
            let (cn, wf) = (1.0 - smooth(0.0, z[1].max(0.03), duh.abs()), if k == 0 { 1.0 - inland(xu(xh)) } else { 0.0 });
            if rnd(hid, ly + 6.0, 1611.0) <= dens * mix(0.35, 1.0, cn.sqrt()) + 0.4 * wf && duh.abs() + (hw + 1.0) / (TAU * dk) <= z[1] {
                hs.push(House { blk, hid, hw, xh, cn, wf, lamps: vec![] });
            }
        }
        // a lamp goes with the house it stands in front of, else the next one to its right, else the one on its left (its pole
        // and light come after that house's parts); lone lamps get their own instance
        let mut lone = vec![];
        for l in lamps.iter() {
            let mut h = hs.iter().position(|q| (l.xl - q.xh).abs() <= q.hw + 0.5).or_else(|| hs.iter().position(|q| q.xh - q.hw > l.xl && q.xh - q.hw - l.xl < 15.0));
            if h.is_none() { if let Some(i) = hs.iter().rposition(|q| q.xh + q.hw < l.xl) { if l.xl - hs[i].xh - hs[i].hw < 15.0 { h = Some(i); } } }
            match h { Some(i) => hs[i].lamps.push(*l), None => lone.push(*l) }
        }
        for hh in &hs {
            let House { blk, hid, hw, xh, cn, wf, .. } = *hh;
            let (uh, id) = (xu(xh), hid + kf * 300.0 + 2000.0);
            let gh = c.town_ground(uh, kf, &s); // level, on the ground at its centre (foundations 4 m into the slope)
            let ns = clamp((1.2 + 2.0 * cn + 1.6 * rnd(hid, ly + 7.0, 1611.0) + wf * (0.6 + 1.2 * rnd(hid, ly + 12.0, 1611.0)) + if blk { 0.8 } else { 0.0 }).floor(), 1.0, 5.0);
            let (rr, pf) = (rnd(hid, ly + 8.0, 1611.0), mix(0.38, 0.06, sty) + wf * 0.12 + if blk { 0.2 } else { 0.0 });
            let rt = if rr < pf { if sty < 0.5 && fract(rr * 91.7) < 0.45 { 4 } else { 3 } }
                else if hw < 6.5 && fract(rr * 37.3) < mix(0.25, 0.6, sty) { 0 } else if fract(rr * 53.1) < mix(0.65, 0.4, sty) { 1 } else { 2 };
            let sh = rnd(hid, ly + 9.0, 1611.0) <= wf * 0.6 + (1.0 - wf) * 0.25 * cn * if k <= 1 { 1.0 } else { 0.0 };
            let (wph, r) = (wp * mix(0.3, 1.3, rnd(hid, ly + 10.0, 1611.0)), [0.0, rnd(id, 1.0, 1620.0), rnd(id, 2.0, 1620.0), rnd(id, 3.0, 1620.0), rnd(id, 4.0, 1620.0)]);
            let (fh, he) = (2.85 + 0.35 * r[1], hw + 0.35);
            let hww = ns * fh + 0.6;
            let (rise, yr) = (if rt == 0 { he * mix(0.62, 1.05, r[2]) } else if rt <= 2 { mix(2.6, 4.4, r[3]).min(he * 0.95) } else { 0.9 }, gh + hww - 0.2);
            let roof_y = |ax: f64| match rt { 0 => rise * (1.0 - ax / he), 1 => rise.min((he - ax) * 1.25), 2 => rise, _ => 1.1 };
            let yf = hh.lamps.iter().fold((gh - 4.0).max(0.0), |m, l| m.min(l.gl));
            c.begin(Instance { uc: fract(uh), d, td: town_td(d, mix(0.8, 1.15, rnd(hid, ly + 11.0, 1611.0))), glow: if hh.lamps.is_empty() { 0.0 } else { 1.5 }, ..I0 }, yf, 0.0);
            c.trap(0.0, yf, hw, hw, gh + hww - yf, 1.0); // wall, lit by the lamps in front of it
            for l in lamps.iter().filter(|l| (l.xl - xh).abs() < hw + 14.0) { c.part(Wash, [l.xl - xh, l.gl, 6.325, 2.5], [0., if l.quay { 0.014 } else { 0.008 }, 0., l.kw]); }
            match rt { // gable to the front, hipped, gabled to the side (seen eave-on), flat with a parapet
                0 => c.trap(0.0, yr - 0.05, he * (1.0 + 0.05 / rise), 0.0, rise + 0.05, 1.0), 1 => c.trap(0.0, yr - 0.05, he + 0.04, he - rise / 1.25, rise + 0.05, 1.0),
                2 => c.trap(0.0, yr - 0.05, he, he, rise + 0.05, 1.0), _ => c.trap(0.0, yr - 0.05, hw, hw, 1.15, 1.0),
            }
            let (cx, chw, yc) = ((r[4] * 2.0 - 1.0) * hw * 0.55, mix(0.3, 0.45, fract(r[4] * 3.3)), gh + hww - 1.0);
            let chim = |c: &mut Ctx, x: f64, top: f64| c.trap(x, yc, chw, chw, top - yc, 1.0);
            if rt <= 2 {
                if r[3] > 0.25 { chim(c, cx, yr + roof_y(cx.abs()) + mix(0.9, 1.6, fract(r[4] * 7.7))); }
                if hw > 5.5 && r[1] > 0.5 { let c2 = -cx * 0.8 + (-cx + 1e-3).signum() * 1.5; chim(c, c2, yr + roof_y(c2.abs()) + 1.0); }
            } else if rt == 4 { // Venetian funnel chimneys
                for (x, y) in [(cx, gh + hww + 2.6 - 1.4), (-0.7 * cx, gh + hww + 2.6 - 1.6)].into_iter().take(if hw > 5.0 { 2 } else { 1 }) {
                    chim(c, x, y); c.trap(x, y, chw, 2.4 * chw, 1.4, 1.0);
                }
            } else if r[3] > 0.55 { c.trap(cx * 0.6, yc, 1.7, 1.7, 3.4, 1.0); } // a stair house on the flat roof
            // windows: whole cells inside the wall with a solid margin, floors of fh, sills .9 m up; only the lit ones exist
            let mg = 0.9f64.max(0.18 * hw);
            let wi = 2.0 * hw - 2.0 * mg;
            let nc = 1f64.max((wi / mix(2.6, 3.4, r[3]) + 0.5).floor());
            let (cw, gsz) = (wi / nc, if r[2] >= 0.55 { 2.0 } else { 1.0 });
            for (fi, ci2) in (0..ns as i32).flat_map(|f| (0..nc as i32).map(move |ci| (f as f64, ci as f64))) {
                let shop = sh && fi == 0.0;
                let (rl, rc, lim) = (rnd(id, 10.0 + fi * 16.0 + (ci2 / gsz).floor(), 1621.0), // rooms share a state
                    rnd(id, 11.0 + fi * 16.0 + ci2, 1621.0), wph * if shop { 1.8 } else { 1.0 });
                if rl + 1e-4 > lim { continue; }
                let (wb, wwh) = (0.3 + fi * fh + if shop { 0.55 } else { 0.9 }, (mix(0.55, 0.7, r[1]) + if shop { 0.4 } else { 0.0 }).min(0.5 * cw - 0.32).max(0.1));
                let wt = wb + if shop { 2.0 } else { 1.45 };
                c.part(Box, [-hw + mg + (ci2 + 0.5) * cw, gh + 0.5 * (wb + wt), wwh, 0.5 * (wt - wb)], [0., 0.5 * (0.55 + 0.55 * fract(rc * 13.7)), 0., if rc >= 0.9 { 1.5 } else { 0.8 }]);
            }
            if rt == 0 && rise > 2.2 { if rnd(id, 5.0, 1621.0) + 1e-4 <= wph * 0.6 { c.part(Box, [0., yr + rise * 0.3, 0.42, 0.5], [0., 0.4, 0., 0.8]); } } // an attic window
            else if rt <= 2 && rise > 3.0 { // dormers in a roof that slopes away
                for (ci2, x) in (0..nc as i32).map(|ci| (ci as f64, -hw + mg + (ci as f64 + 0.5) * cw)) {
                    if x.abs() <= he - 2.2 && rnd(id, 6.0 + ci2, 1621.0) + 1e-4 <= wph * 0.45 { c.part(Box, [x, gh + hww + 0.75, 0.45, 0.5], [0., 0.4, 0., 0.8]); }
                }
            }
            for l in &hh.lamps { lamp(c, l, l.xl - xh); }
            c.end(0.5, 0.5);
        }
        for l in &lone { c.begin(Instance { uc: fract(l.ul), d, glow: 1.5, ..I0 }, l.gl, 0.0); lamp(c, l, 0.0); c.end(0.5, 0.5); }
    }
}
