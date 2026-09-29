//! The flight HUD (`game.hud_flight`): a fighter head-up display drawn over
//! the pane — pitch ladder, flight-path marker, boresight, speed / altitude
//! / heading tapes, bank scale, load factor, weapons, target boxes with the
//! radar lock, the gun pipper and the warnings.
//!
//! The host (which owns the camera and the world) projects everything into
//! pane-normalised coordinates and hands over one [`FlightHud`]; this file
//! only draws, with the HUD's own shape and text draws.

use super::*;
use makepad_scene::HudElement;

/// A point in the pane, 0..1 on both axes (y down).
pub type HudPoint = Vec2f;

/// One rung of the pitch ladder: its pitch in degrees and the four ends of
/// its two halves (outer-left, inner-left, inner-right, outer-right).
#[derive(Clone, Debug, Default)]
pub struct FlightRung {
    pub deg: i32,
    pub ends: [HudPoint; 4],
}

#[derive(Clone, Debug, Default)]
pub struct FlightTarget {
    /// Screen position; None when behind the camera.
    pub at: Option<HudPoint>,
    pub dist: f32,
    pub label: String,
    pub friend: bool,
    /// The radar's target (drawn big, with the lock ring).
    pub selected: bool,
    /// Lock progress 0..1 and locked.
    pub lock: f32,
    pub locked: bool,
    /// A missile, not an aircraft (drawn as a small diamond).
    pub missile: bool,
    /// Direction to it on the pane from the centre (for the edge arrow),
    /// radians, 0 = up, clockwise.
    pub bearing: f32,
}

/// Everything the flight HUD shows this frame.
#[derive(Clone, Debug, Default)]
pub struct FlightHud {
    /// Display airspeed and altitude, already in display units.
    pub speed: f32,
    pub alt: f32,
    pub heading: f32,
    pub pitch: f32,
    pub roll: f32,
    pub vs: f32,
    pub g: f32,
    pub g_peak: f32,
    pub aoa: f32,
    pub mach: f32,
    pub throttle: f32,
    pub burner: f32,
    pub gear: f32,
    pub flap: f32,
    pub airbrake: f32,
    pub stall: f32,
    /// Radar altitude (m over the ground), shown when low.
    pub radar_alt: f32,
    pub boresight: Option<HudPoint>,
    pub fpm: Option<HudPoint>,
    pub ladder: Vec<FlightRung>,
    pub targets: Vec<FlightTarget>,
    pub pipper: Option<HudPoint>,
    pub gun_ammo: i32,
    pub missiles: i32,
    pub flares: i32,
    /// 0..1 missile-rail readiness.
    pub rail: f32,
    /// A missile is guiding on us, and how far it is (m).
    pub threat: f32,
    pub threat_bearing: f32,
    pub pull_up: bool,
    /// The easy-mode assist has the stick (pulling out of terrain).
    pub assist: bool,
    /// First-person (cockpit) view: the full HUD. Otherwise the chase set
    /// (tapes, targets, weapons; no ladder clutter over the aircraft).
    pub cockpit: bool,
    /// Wall time for blinking.
    pub time: f32,
    pub speed_unit: String,
    pub alt_unit: String,
}

struct Pen<'p, 'c, 'e, 'd> {
    cx: &'p mut Cx2d<'c, 'e>,
    draws: &'p mut HudDraws<'d>,
    rect: Rect,
    s: f64,
}

impl Pen<'_, '_, '_, '_> {
    fn px(&self, p: HudPoint) -> (f64, f64) {
        (self.rect.pos.x + p.x as f64 * self.rect.size.x, self.rect.pos.y + p.y as f64 * self.rect.size.y)
    }

    fn line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, w: f64, color: Vec4f) {
        let half = w * 0.5 + 1.0;
        let d = &mut self.draws.shape;
        d.shape = if (x1 - x0) * (y1 - y0) >= 0.0 { 2.0 } else { 3.0 };
        d.fill = color;
        d.thickness = w as f32;
        d.border = 0.0;
        d.radius = 0.0;
        d.draw_abs(
            self.cx,
            Rect {
                pos: dvec2(x0.min(x1) - half, y0.min(y1) - half),
                size: dvec2((x1 - x0).abs() + half * 2.0, (y1 - y0).abs() + half * 2.0),
            },
        );
    }

    /// Axis-aligned hairline box (unfilled).
    fn frame(&mut self, x: f64, y: f64, w: f64, h: f64, stroke: Vec4f, width: f64, fill: Vec4f) {
        let d = &mut self.draws.shape;
        d.shape = 0.0;
        d.fill = fill;
        d.stroke = stroke;
        d.border = width as f32;
        d.radius = 0.0;
        d.draw_abs(self.cx, Rect { pos: dvec2(x, y), size: dvec2(w, h) });
    }

    fn ring(&mut self, x: f64, y: f64, r: f64, width: f64, frac: f32, from: f32, color: Vec4f) {
        let d = &mut self.draws.shape;
        d.shape = 1.0;
        d.fill = color;
        d.thickness = width as f32;
        d.from = from;
        d.sweep = std::f32::consts::TAU;
        d.frac = frac;
        let e = r + width;
        d.draw_abs(self.cx, Rect { pos: dvec2(x - e, y - e), size: dvec2(e * 2.0, e * 2.0) });
    }

    fn text(&mut self, x: f64, y: f64, size: f64, color: Vec4f, text: &str, align: f64) {
        let t = &mut self.draws.text;
        t.text_style.font_size = (size * self.s).max(7.0) as f32;
        t.color = color;
        let w = if align != 0.0 {
            let l = t.layout(self.cx, 0.0, 0.0, None, false, Align::default(), text);
            l.size_in_lpxs.width as f64
        } else {
            0.0
        };
        let h = t.text_style.font_size as f64;
        t.draw_abs(self.cx, dvec2(x - w * align, y - h * 0.62), text);
    }
}

fn with_alpha(c: Vec4f, a: f32) -> Vec4f {
    vec4(c.x, c.y, c.z, c.w * a)
}

/// Draw one flight HUD element over the pane.
pub fn draw_flight(cx: &mut Cx2d, rect: Rect, e: &HudElement, draws: &mut HudDraws, hud: &FlightHud) {
    let s = (rect.size.y / 1080.0).max(0.35);
    let ink = if e.color.w > 0.0 { e.color } else { vec4(0.36, 1.0, 0.62, 0.95) };
    let warn = if e.low_color.w > 0.0 { e.low_color } else { vec4(1.0, 0.25, 0.28, 1.0) };
    let friend = vec4(0.35, 0.72, 1.0, 0.95);
    let shade = vec4(0.0, 0.02, 0.01, 0.55);
    let lw = 2.0 * s;
    let mut pen = Pen { cx, draws, rect, s };
    let cxp = rect.pos.x + rect.size.x * 0.5;
    let cyp = rect.pos.y + rect.size.y * 0.5;
    let blink = (hud.time * 4.0).fract() < 0.55;

    // ---- pitch ladder (cockpit) ----------------------------------------
    if hud.cockpit {
        let (bx, by) = hud.boresight.map_or((cxp, cyp), |b| pen.px(b));
        let window = 300.0 * s;
        for rung in &hud.ladder {
            let pts: Vec<(f64, f64)> = rung.ends.iter().map(|p| pen.px(*p)).collect();
            let mid_y = (pts[1].1 + pts[2].1) * 0.5;
            let mid_x = (pts[1].0 + pts[2].0) * 0.5;
            let d = ((mid_x - bx).powi(2) + (mid_y - by).powi(2)).sqrt();
            if d > window {
                continue;
            }
            let fade = (1.0 - (d / window).powi(3)) as f32;
            let c = with_alpha(ink, fade);
            let w = if rung.deg == 0 { lw * 1.2 } else { lw * 0.8 };
            if rung.deg < 0 {
                // Below the horizon: dashed halves.
                for (a, b) in [(pts[0], pts[1]), (pts[2], pts[3])] {
                    for k in 0..3 {
                        let t0 = k as f64 / 3.0;
                        let t1 = t0 + 0.2;
                        pen.line(a.0 + (b.0 - a.0) * t0, a.1 + (b.1 - a.1) * t0, a.0 + (b.0 - a.0) * t1, a.1 + (b.1 - a.1) * t1, w, c);
                    }
                }
            } else {
                pen.line(pts[0].0, pts[0].1, pts[1].0, pts[1].1, w, c);
                pen.line(pts[2].0, pts[2].1, pts[3].0, pts[3].1, w, c);
            }
            if rung.deg != 0 {
                // End ticks toward the horizon, and the numbers.
                let (dx, dy) = (pts[1].0 - pts[0].0, pts[1].1 - pts[0].1);
                let l = (dx * dx + dy * dy).sqrt().max(1e-3);
                let (nx, ny) = (-dy / l, dx / l);
                let sign = if rung.deg > 0 { 1.0 } else { -1.0 };
                let tick = 10.0 * s * sign;
                pen.line(pts[0].0, pts[0].1, pts[0].0 + nx * tick, pts[0].1 + ny * tick, w, c);
                pen.line(pts[3].0, pts[3].1, pts[3].0 + nx * tick, pts[3].1 + ny * tick, w, c);
                let label = format!("{}", rung.deg.abs());
                pen.text(pts[0].0 - 8.0 * s, pts[0].1, 15.0, c, &label, 1.0);
                pen.text(pts[3].0 + 8.0 * s, pts[3].1, 15.0, c, &label, 0.0);
            }
        }
        // Boresight: the gun cross ("W").
        let w = 14.0 * s;
        pen.line(bx - w * 2.0, by, bx - w, by, lw, ink);
        pen.line(bx - w, by, bx - w * 0.5, by + w * 0.6, lw, ink);
        pen.line(bx - w * 0.5, by + w * 0.6, bx, by, lw, ink);
        pen.line(bx, by, bx + w * 0.5, by + w * 0.6, lw, ink);
        pen.line(bx + w * 0.5, by + w * 0.6, bx + w, by, lw, ink);
        pen.line(bx + w, by, bx + w * 2.0, by, lw, ink);
    }
    // Flight path marker: where the aircraft is actually going.
    if let Some(f) = hud.fpm {
        let (x, y) = pen.px(f);
        let r = 9.0 * s;
        pen.ring(x, y, r, lw, 1.0, 0.0, ink);
        pen.line(x - r - 14.0 * s, y, x - r, y, lw, ink);
        pen.line(x + r, y, x + r + 14.0 * s, y, lw, ink);
        pen.line(x, y - r, x, y - r - 10.0 * s, lw, ink);
    } else if !hud.cockpit {
        if let Some(b) = hud.boresight {
            let (x, y) = pen.px(b);
            pen.ring(x, y, 6.0 * s, lw, 1.0, 0.0, ink);
        }
    }

    // The full instrument set (tapes, heading, bank scale) is the COCKPIT
    // HUD, conformal with the view; the chase view keeps the aircraft
    // clear and reads one compact plate instead.
    if hud.cockpit {
        // ---- speed and altitude tapes --------------------------------------
        let tape_h = 300.0 * s;
        let tape_w = 76.0 * s;
        let top = cyp - tape_h * 0.5;
        // The tapes sit either side of the boresight, pulled in on a narrow
        // pane (a split-screen half) so they stay on it.
        let tape_dx = (430.0 * s).min(rect.size.x * 0.5 - 170.0 * s);
        let sx = cxp - tape_dx;
        let ax = cxp + tape_dx;
        for (x, value, step, label_every, left, unit) in [
            (sx, hud.speed, 10.0f32, 50.0f32, true, hud.speed_unit.as_str()),
            (ax, hud.alt, 20.0, 100.0, false, hud.alt_unit.as_str()),
        ] {
            let px_per = tape_h / (step as f64 * 12.0);
            let base = (value / step).floor() * step;
            let spine = if left { x + tape_w } else { x - tape_w };
            pen.line(spine, top, spine, top + tape_h, lw * 0.7, with_alpha(ink, 0.7));
            for k in -7..=7 {
                let v = base + k as f32 * step;
                if v < 0.0 && left {
                    continue;
                }
                let y = cyp - (v - value) as f64 * px_per;
                if y < top || y > top + tape_h {
                    continue;
                }
                let major = (v / label_every).round() * label_every == v;
                let len = if major { 16.0 } else { 9.0 } * s;
                let (x0, x1) = if left { (spine - len, spine) } else { (spine, spine + len) };
                pen.line(x0, y, x1, y, lw * 0.7, ink);
                if major && (y - cyp).abs() > 22.0 * s {
                    let tx = if left { spine - 22.0 * s } else { spine + 22.0 * s };
                    pen.text(tx, y, 15.0, with_alpha(ink, 0.85), &format!("{}", v as i32), if left { 1.0 } else { 0.0 });
                }
            }
            // The value box on the tape's centre line.
            let bw = 92.0 * s;
            let bh = 34.0 * s;
            let bx = if left { spine - bw - 4.0 * s } else { spine + 4.0 * s };
            pen.frame(bx, cyp - bh * 0.5, bw, bh, ink, lw, shade);
            pen.text(bx + bw * 0.5, cyp, 22.0, ink, &format!("{}", value.round() as i32), 0.5);
            pen.text(if left { spine } else { spine }, top - 16.0 * s, 13.0, with_alpha(ink, 0.8), unit, if left { 1.0 } else { 0.0 });
        }
        // Under the speed tape: G, AoA, Mach, throttle.
        let ly = top + tape_h + 26.0 * s;
        let over_g = hud.g > 8.0;
        pen.text(sx + tape_w, ly, 20.0, if over_g { warn } else { ink }, &format!("{:.1}G", hud.g), 1.0);
        pen.text(sx + tape_w, ly + 24.0 * s, 15.0, with_alpha(ink, 0.8), &format!("PEAK {:.1}", hud.g_peak), 1.0);
        pen.text(sx + tape_w, ly + 46.0 * s, 15.0, if hud.stall > 0.3 { warn } else { with_alpha(ink, 0.8) }, &format!("\u{3b1} {:.0}", hud.aoa), 1.0);
        pen.text(sx + tape_w, ly + 68.0 * s, 15.0, with_alpha(ink, 0.8), &format!("M {:.2}", hud.mach), 1.0);
        // Throttle: a short bar, reheat in the warm colour.
        {
            let bx = sx + tape_w - 120.0 * s;
            let by = ly + 90.0 * s;
            let bw = 120.0 * s;
            let bh = 8.0 * s;
            pen.frame(bx, by, bw, bh, with_alpha(ink, 0.8), 1.0 * s, shade);
            let hot = vec4(1.0, 0.62, 0.2, 1.0);
            pen.frame(bx, by, bw * hud.throttle.clamp(0.0, 1.0) as f64, bh, vec4(0.0, 0.0, 0.0, 0.0), 0.0, if hud.burner > 0.1 { hot } else { with_alpha(ink, 0.8) });
            let label = if hud.burner > 0.1 { "AB".to_string() } else { format!("THR {:.0}", hud.throttle * 100.0) };
            pen.text(bx - 8.0 * s, by + bh * 0.5, 14.0, if hud.burner > 0.1 { hot } else { with_alpha(ink, 0.8) }, &label, 1.0);
        }
        // Under the altitude tape: climb rate, radar altitude, gear / flaps.
        pen.text(ax - tape_w, ly, 18.0, ink, &format!("{:+.0} VS", hud.vs), 0.0);
        if hud.radar_alt < 300.0 {
            pen.text(ax - tape_w, ly + 24.0 * s, 16.0, if hud.radar_alt < 60.0 && hud.gear < 0.5 { warn } else { ink }, &format!("R {:.0}", hud.radar_alt), 0.0);
        }
        let mut sys = Vec::new();
        if hud.gear > 0.02 {
            sys.push(if hud.gear > 0.98 { "GEAR DN" } else { "GEAR" });
        }
        if hud.flap > 0.05 {
            sys.push("FLAPS");
        }
        if hud.airbrake > 0.05 {
            sys.push("BRAKE");
        }
        for (k, line) in sys.iter().enumerate() {
            pen.text(ax - tape_w, ly + (48.0 + 22.0 * k as f64) * s, 15.0, with_alpha(ink, 0.85), line, 0.0);
        }

        // ---- heading tape --------------------------------------------------
        {
            let y = rect.pos.y + 70.0 * s;
            let half_w = (260.0 * s).min(rect.size.x * 0.5 - 40.0 * s);
            let px_per = half_w / 35.0;
            pen.line(cxp - half_w, y + 18.0 * s, cxp + half_w, y + 18.0 * s, lw * 0.6, with_alpha(ink, 0.6));
            let base = (hud.heading / 5.0).floor() as i32 * 5;
            for k in -8..=8 {
                let h = base + k * 5;
                let x = cxp + (h as f32 - hud.heading) as f64 * px_per;
                if (x - cxp).abs() > half_w {
                    continue;
                }
                let major = h.rem_euclid(10) == 0;
                pen.line(x, y + 18.0 * s, x, y + if major { 6.0 } else { 11.0 } * s, lw * 0.7, ink);
                if major && (x - cxp).abs() > 34.0 * s {
                    let hh = h.rem_euclid(360);
                    let label = match hh {
                        0 => "N".to_string(),
                        90 => "E".to_string(),
                        180 => "S".to_string(),
                        270 => "W".to_string(),
                        _ => format!("{:02}", hh / 10),
                    };
                    pen.text(x, y - 8.0 * s, 15.0, with_alpha(ink, 0.85), &label, 0.5);
                }
            }
            let bw = 64.0 * s;
            pen.frame(cxp - bw * 0.5, y - 22.0 * s, bw, 30.0 * s, ink, lw, shade);
            pen.text(cxp, y - 7.0 * s, 20.0, ink, &format!("{:03}", (hud.heading.round() as i32).rem_euclid(360)), 0.5);
            // Bank scale under the tape: a pointer on an arc of ±60°.
            let r = 120.0 * s;
            let ccy = y + 36.0 * s + r;
            for deg in [-60, -45, -30, -20, -10, 0, 10, 20, 30, 45, 60] {
                let a = (deg as f64).to_radians();
                let (sx0, sy0) = (cxp + a.sin() * r, ccy - a.cos() * r);
                let len = if deg % 30 == 0 { 12.0 } else { 7.0 } * s;
                pen.line(sx0, sy0, sx0 - a.sin() * len, sy0 + a.cos() * len, lw * 0.7, with_alpha(ink, 0.7));
            }
            let a = (-(hud.roll as f64)).clamp(-70.0, 70.0).to_radians();
            let (px0, py0) = (cxp + a.sin() * (r + 4.0 * s), ccy - a.cos() * (r + 4.0 * s));
            let d = &mut pen.draws.shape;
            d.shape = 4.0;
            d.from = a as f32;
            d.fill = ink;
            d.stroke = vec4(0.0, 0.0, 0.0, 0.0);
            d.border = 0.0;
            let e = 9.0 * s;
            d.draw_abs(pen.cx, Rect { pos: dvec2(px0 - e, py0 - e), size: dvec2(e * 2.0, e * 2.0) });
        }
    } else {
        let bw = 420.0 * s;
        let bh = 40.0 * s;
        let bx = cxp - bw * 0.5;
        let by = rect.pos.y + rect.size.y - 128.0 * s;
        pen.frame(bx, by, bw, bh, with_alpha(ink, 0.55), 1.0 * s, shade);
        let cy = by + bh * 0.5;
        pen.text(bx + 14.0 * s, cy, 19.0, ink, &format!("{} {}", hud.speed.round() as i32, hud.speed_unit), 0.0);
        pen.text(cxp, cy, 19.0, if hud.radar_alt < 60.0 && hud.gear < 0.5 { warn } else { ink }, &format!("{} {}", hud.alt.round() as i32, hud.alt_unit), 0.5);
        pen.text(bx + bw - 14.0 * s, cy, 19.0, if hud.g > 8.0 { warn } else { ink }, &format!("{:.1}G  {:03}", hud.g, (hud.heading.round() as i32).rem_euclid(360)), 1.0);
        // Throttle along the plate's foot, reheat warm.
        let hot = vec4(1.0, 0.62, 0.2, 1.0);
        pen.frame(bx, by + bh - 3.0 * s, bw * hud.throttle.clamp(0.0, 1.0) as f64, 3.0 * s, vec4(0.0, 0.0, 0.0, 0.0), 0.0, if hud.burner > 0.1 { hot } else { with_alpha(ink, 0.8) });
    }

    // ---- targets -------------------------------------------------------
    let edge = |p: (f64, f64)| p.0 < rect.pos.x + 20.0 || p.0 > rect.pos.x + rect.size.x - 20.0 || p.1 < rect.pos.y + 20.0 || p.1 > rect.pos.y + rect.size.y - 20.0;
    // The off-screen arrow points at the radar's target, or at the nearest
    // bandit when nothing is selected — someone to turn toward, always.
    let selected_on = hud.targets.iter().any(|t| t.selected && !t.friend);
    let nearest = hud
        .targets
        .iter()
        .filter(|t| !t.friend && !t.missile)
        .map(|t| t.dist)
        .fold(None, |m: Option<f32>, d| Some(m.map_or(d, |m| m.min(d))));
    let bandit = vec4(1.0, 0.36, 0.30, 0.95);
    for t in &hud.targets {
        // Bandits read red at a glance; friends blue.
        let color = if t.friend { friend } else if t.locked { warn } else { bandit };
        let on = t.at.map(|a| pen.px(a)).filter(|p| !edge(*p));
        match on {
            Some((x, y)) => {
                if t.missile {
                    let r = 7.0 * s;
                    pen.line(x - r, y, x, y - r, lw, warn);
                    pen.line(x, y - r, x + r, y, lw, warn);
                    pen.line(x + r, y, x, y + r, lw, warn);
                    pen.line(x, y + r, x - r, y, lw, warn);
                    continue;
                }
                if t.friend {
                    // A friendly: a small open diamond and the callsign.
                    let r = 8.0 * s;
                    pen.line(x - r, y, x, y - r, lw * 0.8, color);
                    pen.line(x, y - r, x + r, y, lw * 0.8, color);
                    pen.line(x + r, y, x, y + r, lw * 0.8, color);
                    pen.line(x, y + r, x - r, y, lw * 0.8, color);
                    pen.text(x, y + 20.0 * s, 13.0, color, &t.label, 0.5);
                    continue;
                }
                let half = if t.selected { 22.0 } else { 14.0 } * s;
                // Corner brackets: the target box.
                let k = half * 0.45;
                for (sxn, syn) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    let (cxn, cyn) = (x + sxn * half, y + syn * half);
                    pen.line(cxn, cyn, cxn - sxn * k, cyn, lw, color);
                    pen.line(cxn, cyn, cxn, cyn - syn * k, lw, color);
                }
                if t.selected {
                    if t.locked {
                        // Locked: a solid diamond round the box, blinking text.
                        let r = half * 1.55;
                        pen.line(x - r, y, x, y - r, lw * 1.3, warn);
                        pen.line(x, y - r, x + r, y, lw * 1.3, warn);
                        pen.line(x + r, y, x, y + r, lw * 1.3, warn);
                        pen.line(x, y + r, x - r, y, lw * 1.3, warn);
                        if blink {
                            pen.text(x, y - r - 14.0 * s, 16.0, warn, "LOCK", 0.5);
                        }
                    } else if t.lock > 0.0 {
                        // Tracking: a closing circle.
                        let r = half * (2.6 - 1.2 * t.lock as f64);
                        pen.ring(x, y, r, lw, t.lock, -std::f32::consts::FRAC_PI_2, color);
                    }
                }
                let dist = if t.dist >= 1000.0 { format!("{:.1}", t.dist / 1000.0) } else { format!("{:.0}", t.dist) };
                pen.text(x, y + half + 14.0 * s, 13.0, color, &dist, 0.5);
                if t.selected {
                    pen.text(x, y + half + 32.0 * s, 13.0, color, &t.label, 0.5);
                }
            }
            None if !t.friend && !t.missile && (t.selected || (!selected_on && Some(t.dist) == nearest)) => {
                // Off screen: an arrow on a ring round the centre, pointing.
                let r = 180.0 * s;
                let (sn, cs) = (t.bearing as f64).sin_cos();
                let (x, y) = (cxp + sn * r, cyp - cs * r);
                let d = &mut pen.draws.shape;
                d.shape = 4.0;
                d.from = t.bearing;
                d.fill = color;
                d.stroke = vec4(0.0, 0.0, 0.0, 0.8);
                d.border = 1.0;
                let e = 14.0 * s;
                d.draw_abs(pen.cx, Rect { pos: dvec2(x - e, y - e), size: dvec2(e * 2.0, e * 2.0) });
                let dist = if t.dist >= 1000.0 { format!("{:.1}", t.dist / 1000.0) } else { format!("{:.0}", t.dist) };
                pen.text(x, y + 24.0 * s, 13.0, color, &dist, 0.5);
            }
            None => {}
        }
    }
    // Gun pipper: where to put the target for the cannon.
    if let Some(p) = hud.pipper {
        let (x, y) = pen.px(p);
        pen.ring(x, y, 26.0 * s, lw, 1.0, 0.0, with_alpha(ink, 0.9));
        pen.ring(x, y, 2.5 * s, 3.0 * s, 1.0, 0.0, ink);
    }

    // ---- weapons (an unarmed aircraft shows none) ------------------------
    if hud.gun_ammo > 0 || hud.missiles > 0 || hud.flares > 0 {
        let y = rect.pos.y + rect.size.y - 70.0 * s;
        let gun = format!("GUN {}", hud.gun_ammo.max(0));
        let msl = format!("MSL {}", hud.missiles.max(0));
        let flr = format!("FLR {}", hud.flares.max(0));
        pen.text(cxp - 170.0 * s, y, 19.0, if hud.gun_ammo <= 0 { warn } else { ink }, &gun, 0.5);
        pen.text(cxp, y, 19.0, if hud.missiles <= 0 { warn } else { ink }, &msl, 0.5);
        pen.text(cxp + 170.0 * s, y, 19.0, if hud.flares <= 0 { warn } else { ink }, &flr, 0.5);
        // Rail readiness under MSL.
        let bw = 70.0 * s;
        pen.frame(cxp - bw * 0.5, y + 14.0 * s, bw * hud.rail.clamp(0.0, 1.0) as f64, 3.0 * s, vec4(0.0, 0.0, 0.0, 0.0), 0.0, ink);
    }

    // ---- warnings ------------------------------------------------------
    let mut warnings: Vec<String> = Vec::new();
    if hud.threat > 0.0 {
        warnings.push(format!("MISSILE  {:.1}", hud.threat / 1000.0));
    }
    if hud.assist {
        warnings.push("AUTO PULL-UP".into());
    } else if hud.pull_up {
        warnings.push("PULL UP".into());
    }
    if hud.stall > 0.5 {
        warnings.push("STALL".into());
    }
    for (k, w) in warnings.iter().enumerate() {
        if blink || k > 0 {
            let y = cyp + 150.0 * s + k as f64 * 34.0 * s;
            let t = &mut pen.draws.text;
            t.text_style.font_size = (26.0 * s) as f32;
            let l = t.layout(pen.cx, 0.0, 0.0, None, false, Align::default(), w);
            let tw = l.size_in_lpxs.width as f64;
            pen.frame(cxp - tw * 0.5 - 14.0 * s, y - 20.0 * s, tw + 28.0 * s, 40.0 * s, warn, lw, vec4(0.2, 0.0, 0.02, 0.55));
            pen.text(cxp, y, 26.0, warn, w, 0.5);
        }
    }
    if hud.threat > 0.0 {
        // Where the missile is coming from: a red arrow round the centre.
        let r = 230.0 * s;
        let (sn, cs) = (hud.threat_bearing as f64).sin_cos();
        let d = &mut pen.draws.shape;
        d.shape = 4.0;
        d.from = hud.threat_bearing;
        d.fill = warn;
        d.stroke = vec4(0.0, 0.0, 0.0, 0.8);
        d.border = 1.0;
        let e = 16.0 * s;
        let (x, y) = (cxp + sn * r, cyp - cs * r);
        d.draw_abs(pen.cx, Rect { pos: dvec2(x - e, y - e), size: dvec2(e * 2.0, e * 2.0) });
    }
}
