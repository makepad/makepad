// Layout / turtle, headless: a real widget tree (makepad-widgets-core's Views, Labels, Buttons,
// Chips from its typed style library) laid out and drawn into a draw list by the real Cx2d turtle code,
// the way the widgets crate's own unit tests do (Cx::new + init_cx_os, no window, nothing submitted
// to a GPU: the pass and draw list are built, never presented). Per iteration: 12 passes over the
// tree at different pass widths (flow Right/Down, Fill/Fit sizes, wrapped text); the checksum folds
// the laid-out rectangles of the first, middle and last rows.
use crate::fold;
use makepad_widgets_core::makepad_draw::cx_draw::CxDraw;
use makepad_widgets_core::*;

const ROWS: usize = 120;

// Build from the same library presets the original fixed document used. Only the row text
// and IDs vary; neither comes from user source.
fn scene(cx: &mut Cx) -> WidgetRef {
    let mut root = View::new_default(cx);
    root.walk.width = Size::fill();
    root.walk.height = Size::fit();
    root.layout.flow = Flow::Down;
    root.layout.spacing = 4.0;
    for i in 0..ROWS {
        let mut row = View::new_default(cx);
        row.walk.width = Size::fill();
        row.walk.height = Size::fit();
        row.layout.flow = Flow::right();
        row.layout.spacing = 6.0;
        row.layout.align.y = 0.5;
        let mut label = Label::new_default(cx);
        label.set_text(cx, &format!("item number {i}: a label that is long enough to take some width in the row"));
        row.children.push((LiveId(0), WidgetRef::new_with_inner(Box::new(label))));
        let mut column = View::new_default(cx);
        column.walk.width = Size::fill();
        column.walk.height = Size::fit();
        column.layout.flow = Flow::Down;
        let mut label = Label::new_default(cx);
        label.walk.width = Size::fill();
        label.set_text(cx, &format!("second line of text for row {i}, which wraps when the row is narrow enough to need it"));
        column.children.push((LiveId(0), WidgetRef::new_with_inner(Box::new(label))));
        let mut button = Button::new_default(cx);
        button.set_text(cx, &format!("open {i}"));
        column.children.push((LiveId(1), WidgetRef::new_with_inner(Box::new(button))));
        row.children.push((LiveId(1), WidgetRef::new_with_inner(Box::new(column))));
        let mut chip = Chip::new_default(cx);
        chip.set_text(cx, &format!("tag {i}"));
        row.children.push((LiveId(2), WidgetRef::new_with_inner(Box::new(chip))));
        root.children.push((LiveId::from_str(&format!("row{i}")), WidgetRef::new_with_inner(Box::new(row))));
    }
    WidgetRef::new_with_inner(Box::new(root))
}

pub fn setup() -> Box<dyn FnMut() -> u64> {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.init_cx_os();
    makepad_widgets_core::font_policy::register_theme_font_resources_typed(&mut cx);
    let theme = makepad_widgets_core::desktop_style::base_tokens(&mut cx);
    makepad_widgets_core::rr::CxStylesExt::set_theme(&mut cx, theme);
    let root = scene(&mut cx);
    let pass = DrawPass::new(&mut cx);
    let mut draw_list = DrawList2d::new(&mut cx);
    let first = root.widget(&cx, ids!(row0));
    let mid = root.widget(&cx, ids!(row60));
    let last = root.widget(&cx, ids!(row119));
    Box::new(move || {
        let mut h = 0u64;
        for k in 0..12 {
            let size = Vec2d { x: 420.0 + 70.0 * k as f64, y: 900.0 };
            pass.set_size(&mut cx, size);
            {
                let event = DrawEvent::default();
                let mut draw = CxDraw::new(&mut cx, &event);
                let mut cx2d = Cx2d::new(&mut draw);
                cx2d.begin_pass(&pass, None);
                draw_list.begin_always(&mut cx2d);
                cx2d.begin_root_turtle(size, Layout::flow_down());
                root.draw_all(&mut cx2d, &mut Scope::empty());
                cx2d.end_pass_sized_turtle();
                draw_list.end(&mut cx2d);
                cx2d.end_pass(&pass);
            }
            for w in [&first, &mid, &last] {
                let r = w.area().rect(&cx);
                assert!(r.size.x > 0.0 && r.size.y > 0.0, "widget not laid out");
                h = fold(h, (r.pos.x * 64.0) as i64 as u64 ^ ((r.pos.y * 64.0) as i64 as u64) << 20 ^ ((r.size.x * 64.0) as i64 as u64) << 40 ^ ((r.size.y * 64.0) as i64 as u64) << 52);
            }
        }
        h
    })
}
