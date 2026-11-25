//! Property fields keep their trailing action when the inspector narrows.
use makepad_widgets::makepad_draw::cx_draw::CxDraw;
use makepad_widgets::test_cx::{checkout_test_cx, on_test_cx};
use makepad_widgets::*;

fn draw(cx: &mut Cx, pass: &DrawPass, list: &mut DrawList2d, root: &WidgetRef, width: f64) {
    let size = dvec2(width, 400.0);
    pass.set_size(cx, size);
    let event = DrawEvent::default();
    let mut draw = CxDraw::new(cx, &event);
    let mut cx = Cx2d::new(&mut draw);
    cx.begin_pass(pass, None);
    list.begin_always(&mut cx);
    cx.begin_root_turtle(size, Layout::flow_down());
    root.draw_all(&mut cx, &mut Scope::empty());
    cx.end_pass_sized_turtle();
    list.end(&mut cx);
    cx.end_pass(pass);
}

fn parts(cx: &Cx, row: &WidgetRef, control: LiveId) -> [Rect; 5] {
    let value = row.child(id!(value));
    [row.area().rect(cx), row.child(id!(label)).area().rect(cx), value.area().rect(cx),
        row.child(id!(trail)).area().rect(cx), value.child(control).area().rect(cx)]
}

fn assert_title(kind: &str, width: f64, inner: Rect, label: Rect, trail: Option<Rect>) {
    let right = inner.pos.x + inner.size.x;
    assert!((label.pos.x - inner.pos.x).abs() < 1e-8, "{kind} at {width}: label must start at the inner left edge");
    let label_right = label.pos.x + label.size.x;
    if let Some(trail) = trail {
        assert!((trail.pos.x + trail.size.x - right).abs() < 1e-8, "{kind} at {width}: trail must end at the inner right edge");
        assert!((label_right - trail.pos.x).abs() < 1e-8, "{kind} at {width}: label must fill the space up to the trail without overlap");
        assert!((trail.center().y - label.center().y).abs() < 1e-8,
            "{kind} at {width}: trail must be centered on the label line");
    } else {
        assert!((label_right - right).abs() < 1e-8, "{kind} at {width}: label must fill the line with the trail hidden");
    }
}

fn assert_field(kind: &str, width: f64, label: Rect, field: Rect, left: f64, right: f64) {
    assert!(field.pos.y >= label.pos.y + label.size.y, "{kind} at {width}: field must be below the label");
    assert!((field.pos.x - left).abs() < 1e-8, "{kind} at {width}: field must start at the inner left edge");
    assert!((field.pos.x + field.size.x - right).abs() < 1e-8, "{kind} at {width}: field must end at the inner right edge");
}

#[test]
fn property_rows_match_the_old_layout_until_the_field_needs_its_own_line() {
    on_test_cx(|| {
        let mut cx = checkout_test_cx(makepad_widgets_core::script_mod);
        let root = cx.with_vm(|vm| {
            vm.capture_errors();
            let ui = script_eval!(vm, {
                use mod.prelude.widgets.*
                // The old PropRow. Each pair draws at the same origin so even absolute
                // positions and device-pixel rounding can be compared exactly.
                let OldRow = View{
                    width: Fill height: theme.prop_row_height
                    flow: Right align: Align{y: 0.5} spacing: 0
                    label := Label{
                        text: ""
                        width: Fill{weight: 0 basis: "42%" max: theme.prop_label_width}
                        height: Fit
                        padding: Inset{}
                        align: Align{x: 1.0 y: 0.5}
                        flow: Right
                        max_lines: 1
                        text_overflow: TextOverflow.Ellipsis
                        draw_text +: {
                            ink_centered: true
                            color: theme.prop_ink_dim
                            text_style: theme.font_regular{font_size: theme.prop_font_size line_spacing: 1.0}
                        }
                    }
                    gap := View{width: theme.prop_label_gap height: Fill}
                    value := View{width: Fill height: Fill}
                    trail := View{width: 0 height: Fill align: Center}
                }
                let Pair = View{width: Fill height: Fit flow: Overlay}
                View{
                    width: Fill height: Fit flow: Down
                    number := Pair{
                        actual := PropNumberRow{
                            label +: {text: "Scale X"}
                            trail +: {width: 26 TurtleStep{width: 10 height: 10}}
                        }
                        reference := OldRow{
                            label +: {text: "Scale X"}
                            value +: {num := ValueInput{width: Fill height: Fill min_height: 0}}
                            trail +: {width: 26 TurtleStep{width: 10 height: 10}}
                        }
                    }
                    dropdown := Pair{
                        actual := PropDropDownRow{
                            label +: {text: "Mode"}
                            value +: {pick +: {labels: ["Auto" "Manual"]}}
                        }
                        reference := OldRow{
                            label +: {text: "Mode"}
                            value +: {pick := PropDropDown{labels: ["Auto" "Manual"]}}
                        }
                    }
                    checkbox := Pair{
                        actual := PropCheckRow{label +: {text: "Enabled"}}
                        reference := OldRow{
                            label +: {text: "Enabled"}
                            value +: {check := PropCheckBox{}}
                        }
                    }
                    fixed := PropNumberRow{
                        label +: {text: "Y" width: 80}
                        value := Tip{num := ValueInput{width: Fill height: Fill min_height: 0}}
                    }
                    fit_trail := PropNumberRow{
                        label +: {text: "Seed"}
                        trail +: {width: Fit action := ButtonFlat{
                            text: "Reseed" height: Fill margin: Inset{}
                            padding: Inset{left: 4 right: 4}
                        }}
                    }
                    wrapping := PropRow{
                        height: Fit padding: Inset{left: 3 right: 5}
                        label +: {text: "Piece"}
                        trail +: {width: 26 visible: false}
                        value +: {height: Fit flow: Right{wrap: true}
                            first := TurtleStep{width: 50 height: theme.prop_row_height}
                            second := View{width: 50 height: theme.prop_row_height}
                        }
                    }
                    bounded := PropCheckRow{label +: {text: "Enabled" min_width: 40}}
                    group := PropStack{
                        x := PropNumberRow{label +: {text: "X"}}
                        y := PropNumberRow{
                            value_min: 84
                            label +: {text: "Y"}
                            trail +: {width: 26}
                        }
                        z := PropNumberRow{label +: {text: "Z"}}
                        hidden := PropNumberRow{visible: false label +: {width: 320}}
                    }
                }
            });
            let root = WidgetRef::script_from_value(vm, ui);
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "property row setup failed: {errors:?}");
            root
        });
        let mut pass = DrawPass::new(&mut cx);
        pass.set_dpi_factor(&mut cx, 1.0);
        let mut list = DrawList2d::new(&mut cx);
        // In the uncapped range the field has 58% of the width less the gap and trail.
        let value_min: f64 = 59.9;
        let plain_boundary = (value_min + 6.0) / 0.58;
        let trail_boundary = (value_min + 6.0 + 26.0) / 0.58;
        let group_boundary = 200.0; // The override reaches 84 after a label of 84, gap of 6 and trail of 26.
        let cases = [
            ("PropNumberRow", id!(number), id!(num), "num", 26.0, trail_boundary),
            ("PropDropDownRow", id!(dropdown), id!(pick), "pick", 0.0, plain_boundary),
            ("PropCheckRow", id!(checkbox), id!(check), "check", 0.0, plain_boundary),
        ];
        draw(&mut cx, &pass, &mut list, &root, 320.0);
        let fit_row = root.child(id!(fit_trail));
        let fit_width = fit_row.child(id!(trail)).area().rect(&cx).size.x;
        assert!(fit_width > 8.0, "Fit-trail PropNumberRow at 320: trail must include button text and padding");
        let first_fit_wide = ((value_min + 6.0 + fit_width) / 0.58).ceil();

        for width in [320.0, trail_boundary, plain_boundary, 90.0, first_fit_wide - 1.0, first_fit_wide, group_boundary] {
            if width != 320.0 { draw(&mut cx, &pass, &mut list, &root, width); }
            for (kind, pair, control, control_name, trail_width, boundary) in cases {
                let pair = root.child(pair);
                let actual = parts(&cx, &pair.child(id!(actual)), control);
                let reference = parts(&cx, &pair.child(id!(reference)), control);
                if width >= boundary {
                    for ((part, rect), old) in ["row", "label", "value", "trail", control_name]
                        .into_iter().zip(actual).zip(reference) {
                        assert_eq!(rect, old, "{kind} at {width}: {part} moved from its old rect");
                    }
                } else {
                    let [row, label, value, trail, inner] = actual;
                    assert_eq!(label.pos, row.pos, "{kind} at {width}: label must start the first line");
                    assert_title(kind, width, row, label, Some(trail));
                    assert_field(kind, width, label, value, row.pos.x, row.pos.x + row.size.x);
                    assert_eq!(row.size.y, 2.0 * reference[0].size.y, "{kind} at {width}: row must fit two lines");
                    assert_eq!(value.size.y, reference[0].size.y, "{kind} at {width}: field must be one row high");
                    assert_eq!(trail.size, dvec2(trail_width, label.size.y), "{kind} at {width}: trail must keep its size on the label line");
                    if width == 90.0 {
                        assert_eq!(inner, value, "{kind} at {width}: {control_name} must fill the value slot");
                    }
                }
            }

            let fixed = root.child(id!(fixed));
            let [row, label, value, trail, _] = parts(&cx, &fixed, id!(num));
            if width < 80.0 + 6.0 + value_min {
                let kind = "fixed-label PropNumberRow";
                assert_title(kind, width, row, label, Some(trail));
                // Tip exposes the number field's face, which snaps its edges to device pixels.
                assert_field(kind, width, label, value, row.pos.x.round(), (row.pos.x + row.size.x).round());
            } else {
                assert_eq!(label.size.x, 80.0, "fixed-label PropNumberRow at {width}: label must keep its authored width");
            }

            let [row, label, field, trail, _] = parts(&cx, &fit_row, id!(num));
            let kind = "Fit-trail PropNumberRow";
            assert!((trail.size.x - fit_width).abs() < 1e-8, "{kind} at {width}: trail must keep the button's natural width");
            if width - (width * 0.42).min(120.0) - 6.0 - fit_width < value_min {
                assert_title(kind, width, row, label, Some(trail));
                assert_field(kind, width, label, field, row.pos.x, row.pos.x + row.size.x);
                let button = fit_row.child(id!(trail)).child(id!(action)).area().rect(&cx);
                // Button faces are stored in the draw list at single precision.
                assert!((button.pos.x + button.size.x - row.pos.x - row.size.x).abs() < 1e-4,
                    "{kind} at {width}: button must end at the row's inner right edge");
                assert_eq!(button.pos.y, label.pos.y, "{kind} at {width}: button must sit on the label line");
            } else {
                assert_eq!(field.pos.y, row.pos.y, "{kind} at {width}: field must stay inline when the button fits");
            }

            let wrapping = root.child(id!(wrapping));
            let [mut inner, label, value, _, first] = parts(&cx, &wrapping, id!(first));
            inner.pos.x += 3.0;
            inner.size.x -= 8.0;
            if inner.size.x < plain_boundary {
                let kind = "wrapping PropRow";
                assert_title(kind, width, inner, label, None);
                assert_field(kind, width, label, value, inner.pos.x, inner.pos.x + inner.size.x);
                if inner.size.x < 100.0 {
                    let second = wrapping.child(id!(value)).child(id!(second)).area().rect(&cx);
                    assert!(second.pos.y >= first.pos.y + first.size.y, "{kind} at {width}: second control must wrap below the first");
                    assert_eq!(value.pos.y + value.size.y, second.pos.y + second.size.y, "{kind} at {width}: field must grow to fit the wrapped control");
                    assert_eq!(inner.pos.y + inner.size.y, value.pos.y + value.size.y, "{kind} at {width}: row must contain the full field");
                }
            }

            let bounded = root.child(id!(bounded));
            assert_eq!(bounded.child(id!(value)).area().rect(&cx).pos.y, bounded.area().rect(&cx).pos.y,
                "bounded PropCheckRow at {width}: an unpriced label constraint must keep the original layout");

            let group = root.child(id!(group));
            let mut left = None;
            for (id, trail_width) in [(id!(x), 0.0), (id!(y), 26.0), (id!(z), 0.0)] {
                let [row, label, field, trail, _] = parts(&cx, &group.child(id), id!(num));
                let kind = "PropStack PropNumberRow";
                assert_eq!(field.pos.x, *left.get_or_insert(field.pos.x), "{kind} {id} at {width}: fields must share their left edge");
                if width < group_boundary {
                    assert_title(kind, width, row, label, Some(trail));
                    assert_field(kind, width, label, field, row.pos.x, row.pos.x + row.size.x);
                    assert_eq!(trail.size.x, trail_width, "{kind} {id} at {width}: trail must keep its authored width");
                } else {
                    assert_eq!(field.pos.y, row.pos.y, "{kind} {id} at {width}: all visible fields must return to the wide layout");
                }
            }
        }
    });
}
