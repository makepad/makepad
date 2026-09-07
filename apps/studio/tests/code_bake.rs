//! Standalone code-editor-level gate, kept here to respect this lane's file scope.
//! Studio's macOS appearance module cannot compile with `--cfg headless`.
//! Build makepad-code-editor with MAKEPAD=headless RUSTFLAGS='--cfg headless',
//! then compile this file with rustc --test --cfg headless and those release
//! makepad_code_editor/makepad_widgets rlibs. No Studio library is referenced.
//! Like widgets/tests/gauss_chain.rs the gate is ignored on ordinary native runs.
//! This also permits native `cargo check --all-targets` without an undeclared
//! custom cfg warning (Studio has no build script declaring cfg(headless)).

#[path = "../src/atlas/code_bake.rs"]
pub mod code_bake;

use code_bake::*;
use makepad_code_editor::{
    document::AnchorRange,
    text::{Drift, Position},
    CodeDocument, CodeSession,
};
use makepad_widgets::*;
use std::{cell::RefCell, rc::Rc, sync::Arc};

const FIXTURE: &str = "// tabs and Unicode must match the live editor\n#[derive(Debug)]\nfn main() {\n\tlet café = \"λ 日本語\";\n\tprintln!(\"{}\", café);\n\tlet answer: u32 = 42;\n}\n";

struct Gate {
    baker: CodeBaker,
    live: makepad_code_editor::CodeEditor,
    session: CodeSession,
    source: Arc<str>,
    key: Option<StripId>,
    pass: DrawPass,
    list: DrawList2d,
    live_texture: Texture,
    consumer_texture: Texture,
    consumer: DrawPass,
    consumer_list: DrawList2d,
    quad: DrawCodeBake,
    draws: usize,
    baked: usize,
    serial: u64,
    pitch: BakeLevel,
    dpi: f64,
    origin: Vec2d,
    range: std::ops::Range<u32>,
    reference_size: Vec2d,
    request_frame: bool,
}

fn target(
    cx: &mut Cx,
    name: &str,
    size: (usize, usize),
    dpi: f64,
) -> (DrawPass, Texture, DrawList2d) {
    let pass = DrawPass::new_with_name(cx, name);
    pass.set_live_with_parent(cx, false);
    pass.set_size(cx, dvec2(size.0 as f64 / dpi, size.1 as f64 / dpi));
    let texture = Texture::new_with_format(
        cx,
        TextureFormat::RenderBGRAu8 {
            size: TextureSize::Fixed {
                width: size.0,
                height: size.1,
            },
            initial: true,
        },
    );
    pass.set_color_texture(
        cx,
        &texture,
        DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
    );
    (pass, texture, DrawList2d::new(cx))
}

impl Gate {
    fn new(
        cx: &mut Cx,
        level: BakeLevel,
        dpi: f64,
        origin: Vec2d,
        range: std::ops::Range<u32>,
    ) -> Self {
        cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            makepad_code_editor::script_mod(vm);
        });
        let source: Arc<str> = FIXTURE.repeat(18).into();
        // Reference preparation is intentionally independent of the baker. This
        // test fixture is tiny; production baking only uses its worker path.
        let document = CodeDocument::new(source.as_ref().into(), Default::default());
        let mut session = CodeSession::new(document.clone());
        session.set_view_range(Some(AnchorRange {
            start: document.create_anchor(
                Position {
                    line_index: range.start as usize,
                    byte_index: 0,
                },
                Drift::Before,
            ),
            end: document.create_anchor(
                Position {
                    line_index: range.end as usize,
                    byte_index: 0,
                },
                Drift::After,
            ),
        }));
        let (pass, live_texture, list) = target(cx, "code_bake_live_reference", (1024, 768), dpi);
        let (consumer, consumer_texture, consumer_list) =
            target(cx, "code_bake_consumer", (1024, 768), dpi);
        Self {
            baker: CodeBaker::new(cx).unwrap(),
            live: CodeBaker::new_editor(cx),
            session,
            source,
            key: None,
            pass,
            live_texture,
            list,
            consumer_texture,
            consumer,
            consumer_list,
            quad: DrawCodeBake::new(cx),
            draws: 0,
            baked: 0,
            serial: 0,
            pitch: level,
            dpi,
            origin,
            range,
            reference_size: dvec2(1024.0 / dpi, 768.0 / dpi),
            request_frame: false,
        }
    }

    fn draw(&mut self, cx: &mut Cx2d) {
        self.draws += 1;
        // First draw measures the real font. The second draws the reference at
        // the requested pitch and schedules the immutable snapshot exactly once.
        let mut measured = None;
        if self.draws <= 2 {
            cx.begin_pass(&self.pass, Some(self.dpi));
            self.list.begin_always(cx);
            let size = self.reference_size;
            cx.begin_root_turtle(size, Layout::flow_overlay());
            self.live.draw_walk_editor(
                cx,
                &mut self.session,
                Walk {
                    abs_pos: Some(
                        dvec2(GUARD_PX as f64 / self.dpi, GUARD_PX as f64 / self.dpi) + self.origin,
                    ),
                    width: Size::Fixed(size.x),
                    height: Size::Fixed(size.y),
                    ..Walk::default()
                },
            );
            measured = Some(self.live.metrics(&self.session));
            cx.end_pass_sized_turtle();
            self.list.end(cx);
            cx.end_pass(&self.pass);
        }
        if self.draws == 1 {
            let metrics = measured.unwrap();
            self.live
                .set_font_scale((self.pitch.pitch() / metrics.line_advance) as f32);
            let key = BakeKey {
                digest: self.session.document().digest(),
                document_version: 0,
                language: "rust".into(),
                source_lines: self.range.clone(),
                columns: ColumnPolicy {
                    first: 0,
                    count: 64,
                    gutter: true,
                },
                font: FontKey {
                    identity: "theme.font_code".into(),
                    version: 0,
                },
                metrics: MetricsKey::new(metrics, self.origin),
                theme_generation: 0,
                level: self.pitch,
                dpi_bits: self.dpi.to_bits(),
            };
            let scale = self.pitch.pitch() / metrics.line_advance;
            let width = ((metrics.column_advance * 64.0 + metrics.gutter_width) * scale * self.dpi)
                .ceil() as usize
                + 4;
            let height =
                ((self.range.end - self.range.start) as f64 * self.pitch.pitch() * self.dpi).ceil()
                    as usize
                    + 4;
            let (pass, texture, list) = target(
                cx,
                "code_bake_live_reference_exact",
                (width, height),
                self.dpi,
            );
            self.pass = pass;
            self.live_texture = texture;
            self.list = list;
            self.reference_size = dvec2(width as f64 / self.dpi, height as f64 / self.dpi);
            self.baker
                .set_source(key.digest, 0, self.source.clone())
                .unwrap();
            let ids = self
                .baker
                .schedule(BakeRequest {
                    key,
                    priority: BakePriority::Pointer,
                })
                .unwrap();
            assert_eq!(ids.len(), 1, "fixture range should fit one strip");
            self.baker.retarget(&ids, None, None);
            self.key = Some(ids[0].clone());
        }
        cx.begin_pass(&self.consumer, Some(self.dpi));
        self.consumer_list.begin_always(cx);
        cx.begin_root_turtle(
            dvec2(1024.0 / self.dpi, 768.0 / self.dpi),
            Layout::flow_overlay(),
        );
        // A generous gate budget avoids measuring JIT/font warmup as latency.
        // Production uses DEFAULT_BUDGET_MS and reports measured UI work.
        let report = self.baker.bake_frame(cx, 1000.0);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        self.baked += report.strips_baked;
        self.serial = report.completion_serial;
        self.request_frame = report.request_frame;
        if let Some(strip) = self.key.as_ref().and_then(|id| self.baker.strip(id)) {
            let sample = CodeBaker::sample(Some(strip), None, 0.0).unwrap();
            self.quad.draw_sample(
                cx,
                &sample,
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(
                        strip.physical_size.0 as f64 / self.dpi,
                        strip.physical_size.1 as f64 / self.dpi,
                    ),
                },
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(1.0, 1.0),
                },
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(1.0, 1.0),
                },
            );
        }
        cx.end_pass_sized_turtle();
        self.consumer_list.end(cx);
        cx.end_pass(&self.consumer);
    }
}

fn compare(
    a: &(usize, usize, Vec<u8>),
    b: &(usize, usize, Vec<u8>),
    crop: (usize, usize, usize, usize),
) -> (usize, usize) {
    let (x, y, w, h) = crop;
    let mut different = 0;
    for yy in y..y + h {
        for xx in x..x + w {
            let aa = &a.2[(yy * a.0 + xx) * 4..][..4];
            let bb = &b.2[(yy * b.0 + xx) * 4..][..4];
            if aa.iter().zip(bb).any(|(a, b)| a.abs_diff(*b) > 8) {
                different += 1;
            }
        }
    }
    (different, w * h)
}

#[test]
#[ignore = "requires code-editor libraries built with MAKEPAD=headless and --cfg headless"]
fn baked_strip_matches_live_code_editor_pixels() {
    assert!(
        option_env!("MAKEPAD")
            .unwrap_or_default()
            .contains("headless"),
        "compile the gate against headless libraries"
    );
    std::env::set_var("MAKEPAD_HEADLESS_FRAMES", "off");
    for level in [BakeLevel::L12, BakeLevel::L6, BakeLevel::L3] {
        for (dpi, origin, range) in [
            (1.0, dvec2(0.0, 0.0), 0..14),
            (2.0, dvec2(0.25, 0.25), 31..45),
            (1.0, dvec2(0.0, 0.0), 49..112),
        ] {
            let shared = Rc::new(RefCell::new(None::<Gate>));
            let state = shared.clone();
            let cx = Rc::new(RefCell::new(Cx::new(Box::new(
                move |cx, event| match event {
                    Event::Startup => {
                        if state.borrow().is_none() {
                            *state.borrow_mut() =
                                Some(Gate::new(cx, level, dpi, origin, range.clone()));
                        }
                        cx.redraw_all();
                    }
                    Event::Draw(event) => {
                        let mut draw = CxDraw::new(cx, event);
                        state
                            .borrow_mut()
                            .as_mut()
                            .unwrap()
                            .draw(&mut Cx2d::new(&mut draw));
                    }
                    Event::Signal => cx.redraw_all(),
                    _ => {}
                },
            ))));
            cx.borrow_mut().init_cx_os();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                // Headless single-frame loop is synchronous. It also exercises
                // first-frame pass ordering: the consumer samples a NEW bake.
                Cx::event_loop(cx.clone());
                if shared.borrow().as_ref().unwrap().baked > 0 {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "bake worker timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let mut state = shared.borrow_mut();
            let gate = state.as_mut().unwrap();
            // Ensure reference reached its second (scaled) draw even if the
            // worker completed on the first recording.
            if gate.draws < 2 {
                drop(state);
                Cx::event_loop(cx.clone());
                state = shared.borrow_mut();
            }
            let gate = state.as_mut().unwrap();
            gate.baker.acknowledge_completed(gate.serial);
            let strip = gate.baker.strip(gate.key.as_ref().unwrap()).unwrap();
            let mut context = cx.borrow_mut();
            let baked = context
                .debug_read_render_texture(&strip.texture)
                .expect("bake did not render");
            let live = context
                .debug_read_render_texture(&gate.live_texture)
                .expect("live editor did not render");
            let consumer = context
                .debug_read_render_texture(&gate.consumer_texture)
                .expect("consumer did not render");
            let crop = (
                GUARD_PX as usize,
                GUARD_PX as usize,
                // Restrict to text-containing columns, not the wide empty tail.
                ((strip.content_physical_size.0 as f64 * 0.75) as usize).max(1),
                strip.content_physical_size.1 as usize,
            );
            let (different, pixels) = compare(&baked, &live, crop);
            println!("CodeBaker pixel gate: pitch={} dpi={} origin=({}, {}) lines={:?}: {different}/{pixels} ({:.6}%) differ >8/255",
                level.pitch(), dpi, origin.x, origin.y, strip.source_lines, different as f64 * 100.0 / pixels as f64);
            assert!(
                different as f64 / pixels as f64 <= 0.02,
                "live handoff pixel gate exceeds 2%"
            );
            assert_eq!(
                compare(&baked, &consumer, crop).0,
                0,
                "first-frame consumer must sample produced pixels"
            );
            let unique: std::collections::HashSet<_> = baked
                .2
                .chunks_exact(4)
                .map(|p| [p[0], p[1], p[2], p[3]])
                .collect();
            assert!(unique.len() > 16, "blank captures cannot pass");
            assert!(gate.baker.is_settled());
            assert_eq!(gate.baked, 1);
            assert!(!context.passes[strip.output_pass().draw_pass_id()].live_with_parent);
            let list = context.passes[strip.output_pass().draw_pass_id()]
                .main_draw_list_id
                .unwrap();
            let generation = context.draw_lists[list].redraw_id;
            drop(context);
            drop(state);
            for _ in 0..3 {
                Cx::event_loop(cx.clone());
            }
            assert_eq!(
                cx.borrow().draw_lists[list].redraw_id,
                generation,
                "consumer redraw rebaked a clean strip"
            );
            assert_eq!(shared.borrow().as_ref().unwrap().baked, 1);
            assert!(!shared.borrow().as_ref().unwrap().request_frame);
        }
    }
}
