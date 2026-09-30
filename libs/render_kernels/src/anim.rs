//! Document animation as kernel input (PDOOM-PARITY AK1): a key track
//! (Motion's `keys`: times, values, an ease per key) packed into the
//! buffers `std.anim.track_d` reads, so a kernel, a material or a pass
//! evaluates the same track per element that the document VM evaluates
//! once. Evaluation is f64 like the VM's (Motion's `sample_track`): the
//! linear, hold, polynomial, circ, back, bounce and bezier eases give the
//! VM's bits; sine, expo, elastic and spring use the portable kernels (the
//! VM's host maths agree within an ulp until it uses them too).

/// A key's ease, as `std.ease.apply_d` takes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrackEase {
    /// A named ease: [`ease_id`].
    Id(i32),
    /// A spring from rest over the segment: stiffness and damping.
    Spring { k: f64, d: f64 },
    /// CSS cubic-bezier control points.
    Bezier { x1: f64, y1: f64, x2: f64, y2: f64 },
}

/// The curves in `std.ease.in_curve_d`'s order (Motion's `CURVES`).
pub const CURVES: [&str; 10] = ["quad", "cubic", "quart", "quint", "sine", "expo", "circ", "back", "elastic", "bounce"];

/// `std.ease.apply_d`'s id of a Motion ease name (`linear`, `hold`,
/// `ease_in` / `ease_out` / `ease_in_out` (quad), `ease_{in,out,in_out}_<curve>`,
/// `inout` for `in_out`), or None.
pub fn ease_id(name: &str) -> Option<i32> {
    let name = name.strip_prefix('@').unwrap_or(name);
    match name {
        "linear" => return Some(0),
        "hold" | "step" => return Some(1),
        "ease_in" => return Some(2),
        "ease_out" => return Some(3),
        "ease_in_out" | "ease_inout" => return Some(4),
        _ => {}
    }
    let rest = name.strip_prefix("ease_")?;
    let (dir, curve) = if let Some(c) = rest.strip_prefix("in_out_").or_else(|| rest.strip_prefix("inout_")) {
        (2, c)
    } else if let Some(c) = rest.strip_prefix("in_") {
        (0, c)
    } else {
        (1, rest.strip_prefix("out_")?)
    };
    let c = CURVES.iter().position(|n| *n == curve)? as i32;
    Some(2 + 3 * c + dir)
}

/// A packed track: f64 buffers are low/high word pairs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackBuffers {
    pub times: Vec<u32>,
    pub values: Vec<u32>,
    pub eases: Vec<u32>,
    pub eparams: Vec<u32>,
    /// Keys, and values per key.
    pub keys: usize,
    pub width: usize,
}

fn push_f64(out: &mut Vec<u32>, v: f64) {
    let b = v.to_bits();
    out.push(b as u32);
    out.push((b >> 32) as u32);
}

/// Packs keys `(time, value, ease)` (ascending times; every value
/// `width` long) for `std.anim.track_d`.
pub fn pack_track(keys: &[(f64, &[f64], TrackEase)]) -> TrackBuffers {
    let width = keys.first().map_or(1, |k| k.1.len().max(1));
    let mut t = TrackBuffers { keys: keys.len(), width, ..Default::default() };
    for (time, value, ease) in keys {
        push_f64(&mut t.times, *time);
        for c in 0..width {
            push_f64(&mut t.values, value.get(c).copied().unwrap_or(0.0));
        }
        let (id, p) = match *ease {
            TrackEase::Id(id) => (id, [0.0; 4]),
            TrackEase::Spring { k, d } => (32, [k, d, 0.0, 0.0]),
            TrackEase::Bezier { x1, y1, x2, y2 } => (33, [x1, y1, x2, y2]),
        };
        t.eases.push(id as u32);
        for v in p {
            push_f64(&mut t.eparams, v);
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_names_map_to_ids() {
        assert_eq!(ease_id("@linear"), Some(0));
        assert_eq!(ease_id("ease_in_out"), Some(4));
        assert_eq!(ease_id("ease_in_quad"), Some(2));
        assert_eq!(ease_id("ease_out_bounce"), Some(2 + 27 + 1));
        assert_eq!(ease_id("ease_inout_back"), Some(2 + 21 + 2));
        assert_eq!(ease_id("ease_sideways_quad"), None);
    }
}
