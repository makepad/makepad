//! One test per construct: what the layout puts where, and that every item
//! carries its part.

use makepad_latex_math::{layout, parse, LayoutGlyph, LayoutItem, LayoutOutput, LayoutRule, MathStyle, NO_PART};

const FONT: &[u8] = include_bytes!("../fonts/NewCMMath-Regular.otf");

fn lay(tex: &str, style: MathStyle) -> LayoutOutput {
    layout(&parse(tex), FONT, 100.0, style).expect("layout")
}

fn display(tex: &str) -> LayoutOutput {
    lay(tex, MathStyle::Display)
}

fn glyphs(o: &LayoutOutput) -> Vec<&LayoutGlyph> {
    o.items.iter().filter_map(|i| if let LayoutItem::Glyph(g) = i { Some(g) } else { None }).collect()
}

fn rules(o: &LayoutOutput) -> Vec<&LayoutRule> {
    o.items.iter().filter_map(|i| if let LayoutItem::Rule(r) = i { Some(r) } else { None }).collect()
}

fn glyph_of<'a>(o: &'a LayoutOutput, ch: char) -> &'a LayoutGlyph {
    glyphs(o).into_iter().find(|g| g.ch == ch).unwrap_or_else(|| panic!("no glyph for {ch:?}"))
}

fn part_path(o: &LayoutOutput, g: &LayoutGlyph) -> String {
    o.parts[g.part as usize].path_string()
}

#[test]
fn fraction_bar_sits_between_numerator_and_denominator() {
    let o = display(r"\frac{a}{b}");
    let bar = rules(&o)[0];
    let (a, b) = (glyph_of(&o, 'a'), glyph_of(&o, 'b'));
    assert!(a.y < bar.y, "numerator baseline {} above bar {}", a.y, bar.y);
    assert!(b.y > bar.y + bar.height, "denominator baseline {} below bar {}", b.y, bar.y);
    // both centered on the bar
    let mid = bar.x + bar.width * 0.5;
    assert!((a.x - mid).abs() < 40.0 && (b.x - mid).abs() < 40.0);
}

#[test]
fn script_and_radical_and_accent() {
    let o = display("x^2_i");
    let (x, two, i) = (glyph_of(&o, 'x'), glyph_of(&o, '2'), glyph_of(&o, 'i'));
    assert!(two.y < x.y && i.y > x.y && two.size < x.size);

    // radical with an index: the index glyph sits left of and above the radicand
    let o = display(r"\sqrt[3]{x+1}");
    let (idx, rad) = (glyph_of(&o, '3'), glyph_of(&o, 'x'));
    assert!(idx.x < rad.x && idx.size < rad.size);
    assert_eq!(rules(&o).len(), 1, "one overbar");
    assert!(glyph_of(&o, '\u{221A}').x < rad.x);

    let o = display(r"\hat{x}");
    let (acc, base) = (glyph_of(&o, '\u{302}'), glyph_of(&o, 'x'));
    assert!(acc.x != base.x || acc.y != base.y);
    let o = display(r"\bar{x}");
    assert!(glyphs(&o).iter().any(|g| g.ch == '\u{304}'));
}

#[test]
fn display_sum_puts_limits_above_and_below() {
    let o = display(r"\sum_{k=1}^{N} k");
    let sum = glyph_of(&o, '\u{2211}');
    let n = glyph_of(&o, 'N');
    let one = glyph_of(&o, '1');
    assert!(n.y < sum.y - 20.0, "upper limit above the sum");
    assert!(one.y > sum.y + 20.0, "lower limit below the sum");
    // centered over the sum, not beside it
    assert!(n.x < sum.x + 80.0 && n.x + 10.0 > sum.x - 40.0, "N over the sum, x {} vs {}", n.x, sum.x);
    // inline style: beside
    let o = lay(r"\sum_{k=1}^{N} k", MathStyle::Text);
    let (sum, n) = (glyph_of(&o, '\u{2211}'), glyph_of(&o, 'N'));
    assert!(n.x > sum.x + 30.0);
}

#[test]
fn integral_limits_sit_beside_the_sign() {
    let o = display(r"\int_{0}^{1} f");
    let int = glyph_of(&o, '\u{222B}');
    let one = glyph_of(&o, '1');
    let zero = glyph_of(&o, '0');
    assert!(one.x > int.x + 30.0, "upper limit to the right: {} vs {}", one.x, int.x);
    assert!(zero.x > int.x + 20.0, "lower limit to the right too");
    assert!(one.y < int.y && zero.y > int.y);
    // the upper limit is further right than the lower (italic correction)
    assert!(one.x >= zero.x);
}

#[test]
fn delimiters_grow_with_their_content() {
    let small = display(r"\left( x \right)");
    let tall = display(r"\left( \frac{\frac{a}{b}}{\frac{c}{d}} \right)");
    let paren = |o: &LayoutOutput| -> f32 {
        let p: Vec<_> = glyphs(o).into_iter().filter(|g| g.ch == '(').collect();
        assert!(!p.is_empty());
        p.iter().map(|g| g.size).fold(0.0, f32::max)
    };
    assert_eq!(paren(&small), paren(&tall));
    let small_glyph = glyph_of(&small, '(').glyph_id;
    let tall_glyph = glyph_of(&tall, '(').glyph_id;
    assert_ne!(small_glyph, tall_glyph, "a taller variant is picked");
    // the grown delimiter spans the content: the tall formula's height exceeds the small one's
    assert!(tall.height > small.height * 2.0);
}

#[test]
fn a_very_tall_delimiter_is_assembled_from_parts() {
    let rows = r"a\\b\\c\\d\\e\\f\\g\\h\\i\\j";
    let o = display(&format!(r"\left[ \begin{{matrix}} {rows} \end{{matrix}} \right]"));
    let parts: Vec<_> = glyphs(&o).into_iter().filter(|g| g.ch == '[').collect();
    assert!(parts.len() >= 3, "end pieces plus extenders, got {}", parts.len());
    // the pieces stack without gaps: sorted by y they cover the formula height
    let top = parts.iter().map(|g| g.y).fold(f32::MAX, f32::min);
    let bottom = parts.iter().map(|g| g.y).fold(f32::MIN, f32::max);
    assert!(bottom - top > o.height * 0.6, "stack spans {} of {}", bottom - top, o.height);
    // one part for the whole delimiter
    let first = parts[0].part;
    assert!(parts.iter().all(|g| g.part == first));
}

#[test]
fn matrix_aligned_and_cases() {
    let o = display(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}");
    let (a, b, c) = (glyph_of(&o, 'a'), glyph_of(&o, 'b'), glyph_of(&o, 'c'));
    assert!(b.x > a.x && c.y > a.y && (a.x - c.x).abs() < 30.0);
    assert!(glyphs(&o).iter().any(|g| g.ch == '('));
    assert!(part_path(&o, b).contains("row:0") && part_path(&o, b).contains("col:1"));

    let o = display(r"\begin{aligned} x &= 1 \\ yy &= 22 \end{aligned}");
    let eqs: Vec<_> = glyphs(&o).into_iter().filter(|g| g.ch == '=').collect();
    assert_eq!(eqs.len(), 2);
    assert!((eqs[0].x - eqs[1].x).abs() < 1.0, "the = signs line up: {} {}", eqs[0].x, eqs[1].x);

    let o = display(r"\begin{cases} 1 & x > 0 \\ 0 & \text{else} \end{cases}");
    assert!(glyphs(&o).iter().any(|g| g.ch == '{'));
    let ones: Vec<_> = glyphs(&o).into_iter().filter(|g| g.ch == '1' || g.ch == '0').collect();
    assert!((ones[0].x - ones[2].x).abs() < 1.0, "first column left aligned");
}

#[test]
fn symbols_names_and_text_have_glyphs() {
    let o = display(r"\alpha \beta \infty \partial \pm \cdot \to \approx \leq \geq \sin x \text{if}");
    for ch in ['\u{3B1}', '\u{3B2}', '\u{221E}', '\u{2202}', '\u{B1}', '\u{22C5}', '\u{2192}', '\u{2248}', '\u{2264}', '\u{2265}', 's', 'i', 'n'] {
        glyph_of(&o, ch);
    }
}

#[test]
fn every_item_carries_a_part_and_ids_do_not_move_anything() {
    let plain = display(r"\frac{a+b}{c}");
    let named = display(r"\frac{\id{top}{a+b}}{c}");
    for o in [&plain, &named] {
        assert!(o.items.iter().all(|i| i.part() != NO_PART && (i.part() as usize) < o.parts.len()));
    }
    // \id changes no position
    let pos = |o: &LayoutOutput| glyphs(o).iter().map(|g| (g.glyph_id, g.x, g.y)).collect::<Vec<_>>();
    assert_eq!(pos(&plain), pos(&named));
    assert_eq!(plain.width, named.width);
    // the named part is addressable and holds exactly a, + and b
    let in_top: Vec<char> = glyphs(&named).into_iter().filter(|g| named.parts[g.part as usize].ids == ["top"]).map(|g| g.ch).collect();
    assert_eq!(in_top.len(), 3, "{in_top:?}");
    // paths tell the numerator from the denominator
    let a = glyph_of(&named, 'a');
    let c = glyph_of(&named, 'c');
    assert!(part_path(&named, a).contains("num"), "{}", part_path(&named, a));
    assert!(part_path(&named, c).contains("den"), "{}", part_path(&named, c));
    // the bar belongs to the fraction node, not to either slot
    let bar_part = &named.parts[match named.items.iter().find(|i| matches!(i, LayoutItem::Rule(_))).unwrap() { LayoutItem::Rule(r) => r.part, _ => unreachable!() } as usize];
    assert!(!bar_part.path_string().contains("num") && !bar_part.path_string().contains("den"));
}

#[test]
fn an_id_on_a_big_operator_keeps_its_limits() {
    let a = display(r"\sum_{k}^{N} k");
    let b = display(r"\id{s}{\sum}_{k}^{N} k");
    let pos = |o: &LayoutOutput| glyphs(o).iter().map(|g| (g.glyph_id, g.x, g.y)).collect::<Vec<_>>();
    assert_eq!(pos(&a), pos(&b));
}

#[test]
fn nested_fractions_keep_their_bars_apart() {
    let o = display(r"\frac{\frac{a}{b}}{\frac{c}{d}}");
    let mut bars: Vec<_> = rules(&o).into_iter().map(|r| (r.y, r.height)).collect();
    bars.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    assert_eq!(bars.len(), 3);
    let (top, main, bottom) = (bars[0], bars[1], bars[2]);
    // the inner denominator b sits above the main bar, the inner numerator c below it
    let (b, c) = (glyph_of(&o, 'b'), glyph_of(&o, 'c'));
    assert!(b.y < main.0 - 5.0, "b baseline {} above the main bar {}", b.y, main.0);
    assert!(c.y - c.size * 0.45 > main.0 + main.1, "c top below the main bar");
    assert!(top.0 < main.0 && main.0 < bottom.0);
}

#[test]
fn minus_is_the_minus_sign_and_integral_scripts_tuck_in() {
    let o = display("a-b");
    glyph_of(&o, '-');
    assert_eq!(glyphs(&o).iter().filter(|g| g.ch == '-').count(), 1);
    // the sign in the font is U+2212: its glyph id is not the hyphen's
    let hyphen = layout(&parse("-"), FONT, 100.0, MathStyle::Display).unwrap();
    let ids: Vec<u16> = glyphs(&hyphen).iter().map(|g| g.glyph_id).collect();
    assert_eq!(ids.len(), 1);
    // the integral's lower limit starts left of its upper one
    let o = display(r"\int_{a}^{b}");
    assert!(glyph_of(&o, 'a').x < glyph_of(&o, 'b').x);
}

#[test]
fn aligned_columns_after_spaces() {
    let o = display("\\begin{aligned} x &= 1 \\\\ y &\\ge 2 \\end{aligned}");
    assert!(glyphs(&o).iter().all(|g| g.ch != '&'), "no stray & in the output");
}

/// Ink box (left, top, right, bottom) of a placed glyph, layout units, y down.
fn ink(g: &LayoutGlyph) -> (f32, f32, f32, f32) {
    let ol = makepad_latex_math::glyph_outline(FONT, g.glyph_id, g.size).expect("outline");
    let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut at = |x: f32, y: f32| {
        b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
    };
    for s in ol {
        use makepad_latex_math::OutlineSeg::*;
        match s {
            MoveTo(x, y) | LineTo(x, y) => at(x, y),
            QuadTo(a, c, x, y) => {
                at(a, c);
                at(x, y)
            }
            CurveTo(a, c, d, e, x, y) => {
                at(a, c);
                at(d, e);
                at(x, y)
            }
            Close => {}
        }
    }
    (g.x + b.0, g.y + b.1, g.x + b.2, g.y + b.3)
}

#[test]
fn integral_limits_sit_at_the_signs_top_right_and_bottom_right() {
    for style in [MathStyle::Display, MathStyle::Text] {
        let o = lay(r"\int_{a}^{b}", style);
        let sign = ink(glyph_of(&o, '\u{222B}'));
        let (a, b) = (ink(glyph_of(&o, 'a')), ink(glyph_of(&o, 'b')));
        let em = 100.0;
        // the upper limit starts just right of the sign's ink, level with its top half
        let gap = b.0 - sign.2;
        assert!(gap > -0.05 * em && gap < 0.2 * em, "{style:?}: upper limit gap {gap}");
        assert!(b.3 < (sign.1 + sign.3) * 0.5, "{style:?}: upper limit in the sign's top half");
        // the lower limit is tucked in toward the sign, in its bottom half, left of the upper one
        assert!(a.0 < b.0 - 0.1 * em, "{style:?}: lower {} left of upper {}", a.0, b.0);
        assert!(a.0 > sign.0, "{style:?}: lower limit not left of the sign");
        assert!(a.1 > (sign.1 + sign.3) * 0.5, "{style:?}: lower limit in the sign's bottom half");
        assert!(a.0 - sign.2 < 0.2 * em, "{style:?}: lower limit not far from the sign");
    }
}

#[test]
fn unary_minus_is_ordinary_and_binary_minus_is_spaced() {
    let o = display("a-b");
    let (a, m, b) = (ink(glyph_of(&o, 'a')), ink(glyph_of(&o, '-')), ink(glyph_of(&o, 'b')));
    let binary_gap = b.0 - m.2;
    assert!(m.0 - a.2 > 0.15 * 100.0 && binary_gap > 0.15 * 100.0, "binary minus keeps its spaces");
    // start of a list, after a relation or an opening delimiter
    for tex in [r"-\infty", "a=-b", "(-b)", "a,-b"] {
        let o = display(tex);
        let minus = glyphs(&o).into_iter().find(|g| g.ch == '-').unwrap();
        let next = glyphs(&o).into_iter().filter(|g| g.x > minus.x).min_by(|p, q| p.x.partial_cmp(&q.x).unwrap()).unwrap();
        let (m, n) = (ink(minus), ink(next));
        assert!(n.0 - m.2 < binary_gap - 0.1 * 100.0, "{tex}: unary minus gap {} vs binary {binary_gap}", n.0 - m.2);
    }
}

#[test]
fn script_styles_drop_medium_and_thick_spaces() {
    let em = 70.0;
    let o = display("x^{a=1}");
    let (a, eq, one) = (ink(glyph_of(&o, 'a')), ink(glyph_of(&o, '=')), ink(glyph_of(&o, '1')));
    assert!(eq.0 - a.2 < 0.2 * em && one.0 - eq.2 < 0.2 * em, "relation in a script: {} {}", eq.0 - a.2, one.0 - eq.2);
    let o = display("x^{a+b}");
    let (a, p) = (ink(glyph_of(&o, 'a')), ink(glyph_of(&o, '+')));
    assert!(p.0 - a.2 < 0.2 * em, "binary in a script: {}", p.0 - a.2);
    // at text size the same relation is spaced
    let o = display("a=1");
    let (a, eq) = (ink(glyph_of(&o, 'a')), ink(glyph_of(&o, '=')));
    assert!(eq.0 - a.2 > 0.15 * 100.0);
}

#[test]
fn superscript_sits_close_after_its_base() {
    let o = display("e^{x}");
    let (e, x) = (ink(glyph_of(&o, 'e')), ink(glyph_of(&o, 'x')));
    let gap = x.0 - e.2;
    assert!(gap > 0.0 && gap < 0.1 * 100.0, "gap between e and its superscript: {gap}");
}

#[test]
fn superscripts_on_ascender_letters_sit_as_on_short_ones() {
    let height = |tex: &str| glyph_of(&display(tex), '2').y;
    let (a, x, b, d) = (height("a^2"), height("x^2"), height("b^2"), height("d^2"));
    assert!((a - x).abs() < 0.5, "a {a} x {x}");
    assert!((b - a).abs() < 0.5 && (d - a).abs() < 0.5, "b {b} d {d} vs a {a}");
    // A built base (a fraction) still lifts its script above itself.
    let o = display(r"\left(\frac{a}{b}\right)^2");
    assert!(glyph_of(&o, '2').y < glyph_of(&o, 'a').y);
}

#[test]
fn limit_operators_take_their_limits_underneath_in_display() {
    let o = display(r"\lim_{h \to 0} f");
    let (l, m, h) = (glyph_of(&o, 'l'), glyph_of(&o, 'm'), glyph_of(&o, 'h'));
    assert!(h.y > l.y + 20.0, "the limit sits under the operator: h {} vs lim {}", h.y, l.y);
    assert!(h.x < m.x + 10.0, "and under it, not beside: h {} m {}", h.x, m.x);
    // In text style the limit goes beside, unless \limits says otherwise.
    let side = lay(r"\lim_{h} f", MathStyle::Text);
    assert!(glyph_of(&side, 'h').x > glyph_of(&side, 'm').x);
    let under = lay(r"\lim\limits_{h} f", MathStyle::Text);
    assert!(glyph_of(&under, 'h').y > glyph_of(&under, 'l').y + 20.0);
    // \nolimits puts a display sum's limits beside it.
    let beside = display(r"\sum\nolimits_{k} k");
    let ks: Vec<_> = glyphs(&beside).into_iter().filter(|g| g.ch == 'k').collect();
    assert!(ks[0].x > glyph_of(&beside, '\u{2211}').x + 20.0);
}

#[test]
fn style_switches_and_limits_are_commands_not_text() {
    for tex in [r"\displaystyle \frac{a}{b}", r"\lim\limits_{x} x", r"\textstyle \sum_{k} k", r"\sum\nolimits_k k"] {
        let o = display(tex);
        assert!(!glyphs(&o).iter().any(|g| g.ch == '\\'), "{tex} shows a command as text");
    }
    let size = |tex: &str, style| glyph_of(&lay(tex, style), 'a').size;
    assert!(size(r"\displaystyle \frac{a}{b}", MathStyle::Text) > size(r"\frac{a}{b}", MathStyle::Text));
    let o = display(r"\textstyle \sum_{k} k");
    let ks: Vec<_> = glyphs(&o).into_iter().filter(|g| g.ch == 'k').collect();
    assert!(ks[0].x > glyph_of(&o, '\u{2211}').x + 10.0, "text style: limits beside");
}
