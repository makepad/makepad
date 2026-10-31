// Layout / turtle, headless: a real widget tree (makepad-widgets-core's Views, Labels, Buttons,
// Chips, made by the Splash widget library the way an app's document makes them) laid out and drawn into a draw list by the real Cx2d turtle code,
// the way the widgets crate's own unit tests do (Cx::new + init_cx_os, no window, nothing submitted
// to a GPU: the pass and draw list are built, never presented). Per iteration: 12 passes over the
// tree at different pass widths (flow Right/Down, Fill/Fit sizes, wrapped text); the checksum folds
// the laid-out rectangles of the first, middle and last rows.
use crate::fold;
use makepad_widgets_core::makepad_draw::cx_draw::CxDraw;
use makepad_widgets_core::*;

const ROWS: usize = 120;

fn eval_widget(cx: &mut Cx, source: impl FnOnce(&mut ScriptVm) -> ScriptValue) -> WidgetRef {
    cx.with_vm(|vm| {
        let value = source(vm);
        WidgetRef::script_from_value(vm, value)
    })
}

// A column of 120 rows built by the Splash widget library (the library the apps use), the row
// text varying per row: a Label, a column holding a wrapping Label and a Button, and a Chip.
fn scene(cx: &mut Cx) -> WidgetRef {
    let root = eval_widget(cx, |vm| {
        makepad_widgets_core::script_eval!(vm, {
            use mod.prelude.widgets.*
            use mod.widgets.*
            View{ width: Fill height: Fit flow: Down spacing: 4. }
        })
    });
    for i in 0..ROWS {
        let (t1, t2, t3, t4) = (
            format!("item number {i}: a label that is long enough to take some width in the row"),
            format!("second line of text for row {i}, which wraps when the row is narrow enough to need it"),
            format!("open {i}"),
            format!("tag {i}"),
        );
        let row = eval_widget(cx, |vm| {
            makepad_widgets_core::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill height: Fit flow: Right spacing: 6. align: Align{y: 0.5}
                    Label{ text: #(t1) }
                    View{
                        width: Fill height: Fit flow: Down
                        Label{ width: Fill text: #(t2) }
                        Button{ text: #(t3) }
                    }
                    Chip{ text: #(t4) }
                }
            })
        });
        root.borrow_mut::<View>().unwrap().children.push((LiveId::from_str(&format!("row{i}")), row));
    }
    root
}

pub fn setup() -> Box<dyn FnMut() -> u64> {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.init_cx_os();
    cx.with_vm(makepad_widgets_core::script_mod);
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
