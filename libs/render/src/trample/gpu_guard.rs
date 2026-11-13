//! The trample field on the native GPU, read back texel by texel:
//! - a stamp lands whole in the frame it is drawn: its core is FLAT_MIN
//!   levels flat or more, and nothing beyond its reach is pressed;
//! - the field stands up linearly at any frame rate and completely: one
//!   second at 120 Hz takes 255 / RECOVERY levels off the core (within
//!   RATE_SLACK), and after RECOVERY seconds no texel is over REST_MAX;
//! - a scroll moves it by whole texels: the focus one texel on in x and z
//!   with no time passing, every texel the two frames share is unchanged
//!   (MISMATCH_MAX) and the edge that came in is at rest.
//!
//! Each check's broken arm (a mutation run against this test) is named at
//! its threshold. No time is asserted.
use super::*;
use makepad_platform::makepad_hal::{self as hal_api, open_flags, Hal, OpenReq};
use makepad_platform::ReadbackRequest;
use std::cell::RefCell;
use std::rc::Rc;

const VIEW: Vec2d = Vec2d { x: 16.0, y: 16.0 };
/// A small field: 8 m in 64 texels of 0.125 m.
const TEXELS: usize = 64;
const EXTENT: f32 = 8.0;
const TEXEL: f32 = EXTENT / TEXELS as f32;
/// A stamp's core after its first frame, levels: 255 (all the way); 127 with the stamp
/// blended at half weight.
const FLAT_MIN: u8 = 230;
/// One second at 120 Hz takes 255 / RECOVERY = 31.875 levels off, to within this: 32;
/// 0 with a whole-level decrement (it stalls).
const RATE_SLACK: f64 = 1.5;
/// After RECOVERY seconds no texel is flatter than 2 % of 255: 0; 38 with a whole-level
/// decrement.
const REST_MAX: u8 = 5;
/// Texels a one-texel scroll may change: 0; 169 (of 101 pressed) with the scroll read the
/// other way.
const MISMATCH_MAX: usize = 0;
/// Frames and turns allowed before a step counts as hung (no wall clock).
const TURNS: usize = 4000;

/// One frame's input to the field, and whether to read the target after it.
struct Input {
    focus: Vec3f,
    dt: f32,
    stamps: Vec<GroundStamp>,
    read: bool,
}

struct Rig {
    _window: WindowHandle,
    pass: DrawPass,
    root: DrawList2d,
    field: TrampleField,
    next: Option<Input>,
    ticket: Option<makepad_platform::ReadbackTicket>,
}

/// Close even when an assertion unwinds the test.
struct Session {
    cx: Cx,
    id: u32,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cx.hal_close_session();
        Hal::global().close(self.id);
    }
}

/// The field's texels as RGBA, row j = world z texel j from the corner.
struct Read {
    rgba: Vec<u8>,
}

impl Read {
    fn at(&self, i: usize, j: usize) -> [u8; 3] {
        let k = (j * TEXELS + i) * 4;
        [self.rgba[k], self.rgba[k + 1], self.rgba[k + 2]]
    }
}

/// The texel (i, j) a world point falls in, for a field centred on `focus`.
fn texel_of(focus: Vec3f, x: f32, z: f32) -> (usize, usize) {
    let half = (TEXELS / 2) as i64;
    let origin = [(focus.x / TEXEL).floor() as i64 - half, (focus.z / TEXEL).floor() as i64 - half];
    (((x / TEXEL).floor() as i64 - origin[0]) as usize, ((z / TEXEL).floor() as i64 - origin[1]) as usize)
}

fn stamp(x: f32, z: f32, half: (f32, f32), yaw: f32, push: (f32, f32), strength: f32) -> GroundStamp {
    GroundStamp { pos: vec2(x, z), half_extents: vec2(half.0, half.1), yaw, push: vec2(push.0, push.1), strength }
}

/// Draw one frame with `input`; its target's texels when it asks for them.
fn frame(session: &mut Session, rig: &Rc<RefCell<Option<Rig>>>, input: Input) -> Option<Read> {
    let read = input.read;
    rig.borrow_mut().as_mut().unwrap().next = Some(input);
    session.cx.redraw_all();
    let mut turns = 0;
    while rig.borrow().as_ref().unwrap().next.is_some() {
        turns += 1;
        assert!(turns < TURNS, "the frame was never drawn");
        assert!(session.cx.hal_session_turn(1_000_000, true), "HAL ended drawing a frame");
    }
    if !read {
        return None;
    }
    let ticket = rig.borrow_mut().as_mut().unwrap().ticket.take().expect("the frame asked for its target");
    for _ in 0..TURNS {
        if let Some(result) = session.cx.try_take_texture_readbacks_for(&[ticket]).pop() {
            assert_eq!((result.width, result.height), (TEXELS, TEXELS), "the field's size");
            let data = result.data.expect("the field's readback");
            return Some(Read { rgba: data.to_vec() });
        }
        assert!(session.cx.hal_session_turn(1_000_000, false), "HAL ended reading the field");
    }
    panic!("the field's readback never came");
}

#[test]
fn gpu_guard_trample_lands_recovers_and_scrolls() {
    let rig: Rc<RefCell<Option<Rig>>> = Rc::new(RefCell::new(None));
    let draw_rig = rig.clone();
    let mut cx = Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            cx.with_vm(|vm| {
                vm.capture_errors();
                makepad_platform::script::script_mod(vm);
                makepad_draw::script_mod(vm);
                super::script_mod(vm);
                let errors = vm.take_errors();
                assert!(errors.is_empty(), "trample guard script setup errors: {errors:#?}");
            });
            let mut window = WindowHandle::new(cx);
            window.configure_window(cx, VIEW, dvec2(0.0, 0.0), false, "Trample guard".into());
            let pass = DrawPass::new_with_name(cx, "trample guard window");
            window.set_pass(cx, &pass);
            pass.set_window_clear_color(cx, vec4(0.0, 0.0, 0.0, 1.0));
            *draw_rig.borrow_mut() = Some(Rig {
                _window: window,
                pass,
                root: DrawList2d::new(cx),
                field: TrampleField::new(Some(TrampleBudget { extent: EXTENT, texels: TEXELS })),
                next: None,
                ticket: None,
            });
        }
        if let Event::Draw(event) = event {
            let mut guard = draw_rig.borrow_mut();
            let Some(r) = guard.as_mut() else { return };
            let Some(input) = r.next.take() else { return };
            r.field.set_stamps(input.focus, input.dt, &input.stamps, |_| true);
            {
                let mut draw = CxDraw::new(cx, event);
                let mut cx = Cx2d::new(&mut draw);
                cx.begin_pass(&r.pass, None);
                r.root.begin_always(&mut cx);
                cx.begin_root_turtle(VIEW, Layout::flow_overlay());
                r.field.record(cx.cx);
                cx.end_pass_sized_turtle();
                r.root.end(&mut cx);
                cx.end_pass(&r.pass);
            }
            if input.read {
                let target = r.field.gpu.as_ref().expect("the field ran").targets[r.field.cur].clone();
                r.ticket = Some(target.read_back(cx, ReadbackRequest { next_render: true }).expect("ask for the field"));
            }
        }
    }));
    cx.init_cx_os();
    let req = OpenReq {
        backend: if cfg!(target_vendor = "apple") { hal_api::backend_id::METAL }
            else if cfg!(windows) { hal_api::backend_id::D3D11 }
            else { hal_api::backend_id::AUTO },
        flags: open_flags::OFFSCREEN_WINDOWS | open_flags::RETAIN_WINDOWS,
        dpi_hint: 1.0,
        ..OpenReq::default()
    };
    let info = Hal::global().open(&req);
    assert_ne!(info.session, 0, "the trample guard could not open a GPU session");
    let mut session = Session { cx, id: info.session };
    session.cx.hal_attach_session(info);
    assert_ne!(info.backend, hal_api::backend_id::HEADLESS, "the trample guard needs a real GPU");
    for _ in 0..TURNS {
        if rig.borrow().is_some() { break; }
        assert!(session.cx.hal_session_turn(1_000_000, false), "HAL ended starting");
    }
    assert!(rig.borrow().is_some(), "the guard never started");

    // Warm up far away (a weightless stamp) until both pipelines are in:
    // the field restarts at rest once the feedback can draw.
    let away = vec3f(1000.0, 0.0, 1000.0);
    let ready = |session: &Session| {
        let guard = rig.borrow();
        let f = &guard.as_ref().unwrap().field;
        let shader_in = |dv: &DrawVars| dv.draw_shader_id.is_some_and(|id| session.cx.draw_shader_ready(id, false));
        f.valid && f.gpu.as_ref().is_some_and(|g| shader_in(&g.feedback.draw_vars) && shader_in(&g.stamp.draw_vars))
    };
    let mut warm = 0;
    while !ready(&session) {
        warm += 1;
        assert!(warm < 64, "the trample pipelines never came in ({} pending)", session.cx.pipelines_pending());
        frame(&mut session, &rig, Input { focus: away, dt: 0.0, stamps: vec![stamp(away.x, away.z, (0.5, 0.5), 0.0, (0.0, 0.0), 0.0)], read: false });
        if session.cx.pipelines_pending() > 0 {
            assert!(session.cx.hal_session_turn(u64::MAX, false), "HAL ended awaiting the pipelines");
        }
    }

    let mut broken = Vec::new();
    let tick = 1.0 / 120.0;
    // 1. Lands in one frame.
    let focus = vec3f(0.05, 0.0, 0.05);
    let (sx, sz) = (0.3, -0.2);
    let first = frame(&mut session, &rig, Input { focus, dt: tick, stamps: vec![stamp(sx, sz, (0.5, 0.3), 0.6, (0.8, 0.0), 1.0)], read: true }).unwrap();
    let (ci, cj) = texel_of(focus, sx, sz);
    let core = first.at(ci, cj)[2];
    // Beyond its reach: farther than its half length and a texel from its middle.
    let reach = 0.5 / TEXEL + 1.5;
    let mut stray = 0;
    for j in 0..TEXELS {
        for i in 0..TEXELS {
            let (di, dj) = (i as f32 - ci as f32, j as f32 - cj as f32);
            if (di * di + dj * dj).sqrt() > reach && first.at(i, j)[2] > 0 {
                stray += 1;
            }
        }
    }
    println!("[trample] lands: core {core} levels (min {FLAT_MIN}), {stray} texels pressed beyond its reach");
    if core < FLAT_MIN || stray > 0 {
        broken.push(format!("a stamp lands whole in its frame: core {core} levels (min {FLAT_MIN}), {stray} texels pressed beyond its reach"));
    }

    // 2. Stands up linearly at 120 Hz, and wholly after RECOVERY seconds.
    for k in 0..120 {
        let read = k == 119;
        let got = frame(&mut session, &rig, Input { focus, dt: tick, stamps: Vec::new(), read });
        if let Some(after) = got {
            let off = core as f64 - after.at(ci, cj)[2] as f64;
            let want = 255.0 / RECOVERY;
            println!("[trample] one second at 120 Hz: {off} levels off the core (want {want} +- {RATE_SLACK})");
            if (off - want).abs() > RATE_SLACK {
                broken.push(format!("one second at 120 Hz takes {want} levels off: {off}"));
            }
        }
    }
    let mut last = None;
    for k in 0..(RECOVERY as usize - 1) {
        last = frame(&mut session, &rig, Input { focus, dt: 1.0, stamps: Vec::new(), read: k + 2 == RECOVERY as usize });
    }
    let rested = last.expect("the last recovery frame was read");
    let most = (0..TEXELS * TEXELS).map(|k| rested.at(k % TEXELS, k / TEXELS)[2]).max().unwrap();
    println!("[trample] after {RECOVERY} s: the flattest texel {most} levels (max {REST_MAX})");
    if most > REST_MAX {
        broken.push(format!("after {RECOVERY} s every texel is back under 2 %: the flattest is {most} levels"));
    }

    // 3. A one-texel scroll changes no texel.
    let stamps = vec![
        stamp(-1.2, 0.8, (0.6, 0.25), 0.3, (0.0, 0.9), 1.0),
        stamp(0.9, -1.4, (0.3, 0.7), 2.0, (-0.7, 0.2), 0.8),
        stamp(2.1, 1.9, (0.4, 0.4), 0.0, (0.5, -0.5), 0.6),
    ];
    let before = frame(&mut session, &rig, Input { focus, dt: 0.0, stamps, read: true }).unwrap();
    let moved = vec3f(focus.x + TEXEL, 0.0, focus.z + TEXEL);
    let after = frame(&mut session, &rig, Input { focus: moved, dt: 0.0, stamps: Vec::new(), read: true }).unwrap();
    let pressed = (0..TEXELS * TEXELS).filter(|k| before.at(k % TEXELS, k / TEXELS)[2] > 0).count();
    let mut mismatches = 0;
    let mut edge = 0;
    for j in 0..TEXELS {
        for i in 0..TEXELS {
            if i + 1 < TEXELS && j + 1 < TEXELS {
                if after.at(i, j) != before.at(i + 1, j + 1) {
                    mismatches += 1;
                }
            } else if after.at(i, j) != [128, 128, 0] {
                edge += 1;
            }
        }
    }
    println!("[trample] scroll: {mismatches} of the shared texels changed (max {MISMATCH_MAX}), {edge} new edge texels not at rest, {pressed} pressed before");
    assert!(pressed > 40, "the scroll check needs a pressed field: {pressed} texels");
    if mismatches > MISMATCH_MAX || edge > 0 {
        broken.push(format!("a one-texel scroll changes no texel: {mismatches} shared texels changed, {edge} new edge texels not at rest"));
    }
    assert!(broken.is_empty(), "trample guard:\n{}", broken.join("\n"));
}
