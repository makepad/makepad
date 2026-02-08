/// SVG SMIL animation parser and runtime interpolation.
/// Handles <animate>, <animateTransform>, and value interpolation at a given time.
use makepad_html::HtmlWalker;
use makepad_live_id::*;

use crate::color::parse_color;
use crate::document::*;
use crate::path::VectorPath;
use crate::path_data::parse_path_data;
use crate::units::{parse_duration, parse_number};

// ---- Parsing ----

pub fn parse_animate_element(walker: &HtmlWalker) -> SvgAnimate {
    let mut anim = SvgAnimate::default();

    if let Some(v) = walker.find_attr_lc(live_id!(attributename)) {
        anim.attribute = match v.to_ascii_lowercase().as_str() {
            "fill" => AnimateAttribute::Fill,
            "stroke" => AnimateAttribute::Stroke,
            "stroke-width" => AnimateAttribute::StrokeWidth,
            "opacity" => AnimateAttribute::Opacity,
            "fill-opacity" => AnimateAttribute::FillOpacity,
            "stroke-opacity" => AnimateAttribute::StrokeOpacity,
            "transform" => AnimateAttribute::Transform,
            "d" => AnimateAttribute::D,
            "r" => AnimateAttribute::R,
            "cx" => AnimateAttribute::Cx,
            "cy" => AnimateAttribute::Cy,
            "rx" => AnimateAttribute::Rx,
            "ry" => AnimateAttribute::Ry,
            "x" => AnimateAttribute::X,
            "y" => AnimateAttribute::Y,
            "width" => AnimateAttribute::Width,
            "height" => AnimateAttribute::Height,
            other => AnimateAttribute::Custom(other.to_string()),
        };
    }

    if let Some(v) = walker.find_attr_lc(live_id!(from)) {
        anim.from = Some(v.to_string());
    }
    if let Some(v) = walker.find_attr_lc(live_id!(to)) {
        anim.to = Some(v.to_string());
    }
    if let Some(v) = walker.find_attr_lc(live_id!(values)) {
        anim.values = Some(v.split(';').map(|s| s.trim().to_string()).collect());
    }
    if let Some(v) = walker.find_attr_lc(live_id!(keytimes)) {
        anim.key_times = Some(
            v.split(';')
                .filter_map(|s| s.trim().parse::<f32>().ok())
                .collect(),
        );
    }
    if let Some(v) = walker.find_attr_lc(live_id!(keysplines)) {
        anim.key_splines = Some(parse_key_splines(v));
    }
    if let Some(v) = walker.find_attr_lc(live_id!(dur)) {
        if let Some(d) = parse_duration(v) {
            anim.dur = d;
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(begin)) {
        if let Some(d) = parse_duration(v) {
            anim.begin = d;
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(repeatcount)) {
        anim.repeat_count = parse_repeat_count(v);
    }
    if let Some(v) = walker.find_attr_lc(live_id!(calcmode)) {
        anim.calc_mode = match v.to_ascii_lowercase().as_str() {
            "discrete" => AnimateCalcMode::Discrete,
            "paced" => AnimateCalcMode::Paced,
            "spline" => AnimateCalcMode::Spline,
            _ => AnimateCalcMode::Linear,
        };
    }
    if let Some(v) = walker.find_attr_lc(live_id!(fill)) {
        anim.fill = match v.to_ascii_lowercase().as_str() {
            "freeze" => AnimateFill::Freeze,
            _ => AnimateFill::Remove,
        };
    }

    anim
}

pub fn parse_animate_transform_element(walker: &HtmlWalker) -> SvgAnimateTransform {
    let mut anim = SvgAnimateTransform::default();

    if let Some(v) = walker.find_attr_lc(live_id!(type)) {
        anim.kind = match v.to_ascii_lowercase().as_str() {
            "scale" => AnimateTransformType::Scale,
            "rotate" => AnimateTransformType::Rotate,
            "skewx" => AnimateTransformType::SkewX,
            "skewy" => AnimateTransformType::SkewY,
            _ => AnimateTransformType::Translate,
        };
    }

    if let Some(v) = walker.find_attr_lc(live_id!(from)) {
        anim.from = Some(v.to_string());
    }
    if let Some(v) = walker.find_attr_lc(live_id!(to)) {
        anim.to = Some(v.to_string());
    }
    if let Some(v) = walker.find_attr_lc(live_id!(values)) {
        anim.values = Some(v.split(';').map(|s| s.trim().to_string()).collect());
    }
    if let Some(v) = walker.find_attr_lc(live_id!(keytimes)) {
        anim.key_times = Some(
            v.split(';')
                .filter_map(|s| s.trim().parse::<f32>().ok())
                .collect(),
        );
    }
    if let Some(v) = walker.find_attr_lc(live_id!(dur)) {
        if let Some(d) = parse_duration(v) {
            anim.dur = d;
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(begin)) {
        if let Some(d) = parse_duration(v) {
            anim.begin = d;
        }
    }
    if let Some(v) = walker.find_attr_lc(live_id!(repeatcount)) {
        anim.repeat_count = parse_repeat_count(v);
    }
    if let Some(v) = walker.find_attr_lc(live_id!(calcmode)) {
        anim.calc_mode = match v.to_ascii_lowercase().as_str() {
            "discrete" => AnimateCalcMode::Discrete,
            "paced" => AnimateCalcMode::Paced,
            "spline" => AnimateCalcMode::Spline,
            _ => AnimateCalcMode::Linear,
        };
    }
    if let Some(v) = walker.find_attr_lc(live_id!(fill)) {
        anim.fill = match v.to_ascii_lowercase().as_str() {
            "freeze" => AnimateFill::Freeze,
            _ => AnimateFill::Remove,
        };
    }

    anim
}

fn parse_repeat_count(s: &str) -> RepeatCount {
    let s = s.trim();
    if s.eq_ignore_ascii_case("indefinite") {
        RepeatCount::Indefinite
    } else if let Some(n) = s.parse::<f32>().ok() {
        RepeatCount::Count(n)
    } else {
        RepeatCount::Count(1.0)
    }
}

fn parse_key_splines(s: &str) -> Vec<[f32; 4]> {
    s.split(';')
        .filter_map(|seg| {
            let mut vals = [0.0f32; 4];
            let mut count = 0;
            for p in seg.split(|c: char| c == ',' || c.is_whitespace()) {
                if !p.is_empty() {
                    if count < 4 {
                        vals[count] = p.parse::<f32>().ok()?;
                        count += 1;
                    }
                }
            }
            if count == 4 {
                Some(vals)
            } else {
                None
            }
        })
        .collect()
}

// ---- Runtime Interpolation ----

/// Compute the local time for an animation given a global time.
/// Returns None if the animation hasn't started or has finished (and fill=remove).
pub fn animation_local_time(
    anim_begin: f32,
    anim_dur: f32,
    repeat_count: &RepeatCount,
    fill: &AnimateFill,
    time: f32,
) -> Option<f32> {
    if anim_dur <= 0.0 {
        return None;
    }
    let local = time - anim_begin;
    if local < 0.0 {
        return None;
    }

    let total_dur = match repeat_count {
        RepeatCount::Indefinite => f32::INFINITY,
        RepeatCount::Count(n) => anim_dur * n,
    };

    if local >= total_dur {
        match fill {
            AnimateFill::Freeze => Some(anim_dur), // freeze at end
            AnimateFill::Remove => None,
        }
    } else {
        Some(local % anim_dur)
    }
}

/// Get the normalized progress [0,1] within an animation's duration.
pub fn animation_progress(local_time: f32, dur: f32) -> f32 {
    if dur <= 0.0 {
        return 1.0;
    }
    (local_time / dur).clamp(0.0, 1.0)
}

/// Interpolate between values given a list of values and optional key_times.
/// Returns (value_index_a, value_index_b, segment_t) for blending.
pub fn interpolate_values_index(
    progress: f32,
    num_values: usize,
    key_times: Option<&[f32]>,
) -> (usize, usize, f32) {
    if num_values <= 1 {
        return (0, 0, 0.0);
    }
    let last = num_values - 1;

    if let Some(kt) = key_times {
        if kt.len() == num_values {
            for i in 1..kt.len() {
                if progress <= kt[i] {
                    let range = kt[i] - kt[i - 1];
                    let t = if range > 1e-6 {
                        (progress - kt[i - 1]) / range
                    } else {
                        0.0
                    };
                    return (i - 1, i, t.clamp(0.0, 1.0));
                }
            }
            return (last - 1, last, 1.0);
        }
    }

    // Uniform spacing
    let segment_len = 1.0 / last as f32;
    let idx = (progress / segment_len).floor() as usize;
    let idx = idx.min(last - 1);
    let t = (progress - idx as f32 * segment_len) / segment_len;
    (idx, idx + 1, t.clamp(0.0, 1.0))
}

/// Interpolate a float value.
pub fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Interpolate a color (RGBA, linear).
pub fn lerp_color(
    a: (f32, f32, f32, f32),
    b: (f32, f32, f32, f32),
    t: f32,
) -> (f32, f32, f32, f32) {
    (
        lerp_f32(a.0, b.0, t),
        lerp_f32(a.1, b.1, t),
        lerp_f32(a.2, b.2, t),
        lerp_f32(a.3, b.3, t),
    )
}

/// Evaluate a color animation at a given time.
pub fn eval_color_animation(anim: &SvgAnimate, time: f32) -> Option<(f32, f32, f32, f32)> {
    let local = animation_local_time(anim.begin, anim.dur, &anim.repeat_count, &anim.fill, time)?;
    let progress = animation_progress(local, anim.dur);

    if let Some(ref vals) = anim.values {
        let num_values = vals.len();
        if num_values == 0 {
            return None;
        }
        if num_values == 1 {
            return parse_color(&vals[0]);
        }
        let (ia, ib, t) = interpolate_values_index(progress, num_values, anim.key_times.as_deref());
        let ca = parse_color(&vals[ia]).unwrap_or((0.0, 0.0, 0.0, 1.0));
        let cb = parse_color(&vals[ib]).unwrap_or((0.0, 0.0, 0.0, 1.0));
        Some(lerp_color(ca, cb, t))
    } else {
        let from = anim
            .from
            .as_ref()
            .and_then(|s| parse_color(s))
            .unwrap_or((0.0, 0.0, 0.0, 1.0));
        let to = anim
            .to
            .as_ref()
            .and_then(|s| parse_color(s))
            .unwrap_or((0.0, 0.0, 0.0, 1.0));
        Some(lerp_color(from, to, progress))
    }
}

/// Evaluate a float animation at a given time (for opacity, stroke-width, etc).
pub fn eval_float_animation(anim: &SvgAnimate, time: f32) -> Option<f32> {
    let local = animation_local_time(anim.begin, anim.dur, &anim.repeat_count, &anim.fill, time)?;
    let progress = animation_progress(local, anim.dur);

    if let Some(ref vals) = anim.values {
        let num_values = vals.len();
        if num_values == 0 {
            return None;
        }
        if num_values == 1 {
            return parse_number(&vals[0]);
        }
        let (ia, ib, t) = interpolate_values_index(progress, num_values, anim.key_times.as_deref());
        let va = parse_number(&vals[ia]).unwrap_or(0.0);
        let vb = parse_number(&vals[ib]).unwrap_or(0.0);
        Some(lerp_f32(va, vb, t))
    } else {
        let from = anim
            .from
            .as_ref()
            .and_then(|s| parse_number(s))
            .unwrap_or(0.0);
        let to = anim
            .to
            .as_ref()
            .and_then(|s| parse_number(s))
            .unwrap_or(0.0);
        Some(lerp_f32(from, to, progress))
    }
}

/// Evaluate a transform animation at a given time.
pub fn eval_transform_animation(anim: &SvgAnimateTransform, time: f32) -> Option<Transform2d> {
    let local = animation_local_time(anim.begin, anim.dur, &anim.repeat_count, &anim.fill, time)?;
    let progress = animation_progress(local, anim.dur);

    if let Some(ref vals) = anim.values {
        let num_values = vals.len();
        if num_values == 0 {
            return None;
        }
        if num_values == 1 {
            let (nums, count) = parse_number_list(&vals[0]);
            return Some(numbers_to_transform(&anim.kind, &nums[..count]));
        }
        let (ia, ib, t) = interpolate_values_index(progress, num_values, anim.key_times.as_deref());
        let (va, va_count) = parse_number_list(&vals[ia]);
        let (vb, vb_count) = parse_number_list(&vals[ib]);
        let count = va_count.min(vb_count);
        let mut interpolated = [0.0f32; 3];
        for i in 0..count {
            interpolated[i] = lerp_f32(va[i], vb[i], t);
        }
        Some(numbers_to_transform(&anim.kind, &interpolated[..count]))
    } else {
        let (from, from_count) = anim
            .from
            .as_ref()
            .map(|s| parse_number_list(s))
            .unwrap_or(([0.0; 3], 0));
        let (to, to_count) = anim
            .to
            .as_ref()
            .map(|s| parse_number_list(s))
            .unwrap_or(([0.0; 3], 0));
        let count = from_count.min(to_count);
        if count == 0 {
            return None;
        }
        let mut interpolated = [0.0f32; 3];
        for i in 0..count {
            interpolated[i] = lerp_f32(from[i], to[i], progress);
        }
        Some(numbers_to_transform(&anim.kind, &interpolated[..count]))
    }
}

/// Parse up to 3 floats from a space/comma-separated string (transform values have at most 3: e.g. rotate angle cx cy).
fn parse_number_list(s: &str) -> ([f32; 3], usize) {
    let mut vals = [0.0f32; 3];
    let mut count = 0;
    for p in s.split(|c: char| c == ',' || c.is_whitespace()) {
        if !p.is_empty() && count < 3 {
            if let Ok(v) = p.parse::<f32>() {
                vals[count] = v;
                count += 1;
            }
        }
    }
    (vals, count)
}

fn numbers_to_transform(kind: &AnimateTransformType, nums: &[f32]) -> Transform2d {
    match kind {
        AnimateTransformType::Translate => {
            let tx = nums.first().copied().unwrap_or(0.0);
            let ty = nums.get(1).copied().unwrap_or(0.0);
            Transform2d::translate(tx, ty)
        }
        AnimateTransformType::Scale => {
            let sx = nums.first().copied().unwrap_or(1.0);
            let sy = nums.get(1).copied().unwrap_or(sx);
            Transform2d::scale(sx, sy)
        }
        AnimateTransformType::Rotate => {
            let angle = nums.first().copied().unwrap_or(0.0) * std::f32::consts::PI / 180.0;
            if nums.len() >= 3 {
                let cx = nums[1];
                let cy = nums[2];
                Transform2d::translate(cx, cy)
                    .then(&Transform2d::rotate(angle))
                    .then(&Transform2d::translate(-cx, -cy))
            } else {
                Transform2d::rotate(angle)
            }
        }
        AnimateTransformType::SkewX => {
            let angle = nums.first().copied().unwrap_or(0.0) * std::f32::consts::PI / 180.0;
            Transform2d::skew_x(angle)
        }
        AnimateTransformType::SkewY => {
            let angle = nums.first().copied().unwrap_or(0.0) * std::f32::consts::PI / 180.0;
            Transform2d::skew_y(angle)
        }
    }
}

/// Evaluate path morphing animation (animate d attribute).
/// Returns an interpolated VectorPath by lerping corresponding path commands.
pub fn eval_path_animation(anim: &SvgAnimate, time: f32) -> Option<VectorPath> {
    let local = animation_local_time(anim.begin, anim.dur, &anim.repeat_count, &anim.fill, time)?;
    let progress = animation_progress(local, anim.dur);

    let path_strings: Vec<&str> = if let Some(ref vals) = anim.values {
        vals.iter().map(|s| s.as_str()).collect()
    } else {
        let from = anim.from.as_deref().unwrap_or("");
        let to = anim.to.as_deref().unwrap_or("");
        vec![from, to]
    };

    if path_strings.len() < 2 {
        return None;
    }

    let (ia, ib, t) =
        interpolate_values_index(progress, path_strings.len(), anim.key_times.as_deref());

    let mut path_a = VectorPath::new();
    let mut path_b = VectorPath::new();
    parse_path_data(path_strings[ia], &mut path_a);
    parse_path_data(path_strings[ib], &mut path_b);

    Some(lerp_paths(&path_a, &path_b, t))
}

/// Linearly interpolate two VectorPaths command-by-command.
/// If they have different numbers of commands, uses path_a as fallback.
fn lerp_paths(a: &VectorPath, b: &VectorPath, t: f32) -> VectorPath {
    use crate::path::PathCmd;

    let mut result = VectorPath::new();
    let len = a.cmds.len().min(b.cmds.len());

    for i in 0..len {
        match (&a.cmds[i], &b.cmds[i]) {
            (PathCmd::MoveTo(ax, ay), PathCmd::MoveTo(bx, by)) => {
                result.move_to(lerp_f32(*ax, *bx, t), lerp_f32(*ay, *by, t));
            }
            (PathCmd::LineTo(ax, ay), PathCmd::LineTo(bx, by)) => {
                result.line_to(lerp_f32(*ax, *bx, t), lerp_f32(*ay, *by, t));
            }
            (
                PathCmd::BezierTo(ax1, ay1, ax2, ay2, ax, ay),
                PathCmd::BezierTo(bx1, by1, bx2, by2, bx, by),
            ) => {
                result.bezier_to(
                    lerp_f32(*ax1, *bx1, t),
                    lerp_f32(*ay1, *by1, t),
                    lerp_f32(*ax2, *bx2, t),
                    lerp_f32(*ay2, *by2, t),
                    lerp_f32(*ax, *bx, t),
                    lerp_f32(*ay, *by, t),
                );
            }
            (PathCmd::Close, PathCmd::Close) => {
                result.close();
            }
            // Mismatched commands: use a's command
            _ => {
                result.cmds.push(a.cmds[i].clone());
            }
        }
    }
    // Remaining commands from a
    for i in len..a.cmds.len() {
        result.cmds.push(a.cmds[i].clone());
    }

    result
}
