//! Run with MAKEPAD_HAL_BACKEND=headless for allocation, ticket and lifetime logic.
//! A native backend also exercises the unchanged pixel assertions.
#![cfg(not(target_arch = "wasm32"))]
use makepad_platform::*;
use makepad_platform::os::{ReadbackError, ReadbackRequest, TEXTURE_READBACK_MAX_BYTES};

#[test]
fn released_render_texture_reallocates_fifty_sizes_without_retaining_storage() {
    let (mut cx, session) = open_session();
    let texture = Texture::new_with_format(
        &mut cx,
        TextureFormat::RenderBGRAu8 {
            size: TextureSize::Fixed {
                width: 2048,
                height: 2048,
            },
            initial: true,
        },
    );
    let clone = texture.clone();
    let id = texture.texture_id();
    let pass = DrawPass::new(&mut cx);
    let list = DrawList::new(&mut cx);
    pass.add_color_texture(
        &mut cx,
        &texture,
        DrawPassClearColor::InitWith(vec4(0.25, 0.5, 0.75, 1.0)),
    );
    cx.passes[pass.draw_pass_id()].main_draw_list_id = Some(list.id());
    let baseline = session.texture_bytes();
    assert_eq!(session.allocation_bytes(), None);
    assert_eq!(cx.frame_submission_serial(), 0);
    assert_eq!(cx.frame_completion_serial(), 0);
    let mut last_serial = 0;
    for iteration in 0..51 {
        let size = if iteration == 0 {
            2048
        } else {
            31 + iteration * 7
        };
        let TextureFormat::RenderBGRAu8 { size: extent, .. } = texture.get_format(&mut cx) else {
            unreachable!()
        };
        *extent = TextureSize::Fixed {
            width: size,
            height: size,
        };
        pass.set_size(&mut cx, dvec2(size as f64, size as f64));
        cx.passes[pass.draw_pass_id()].dpi_factor = Some(1.0);
        cx.passes[pass.draw_pass_id()].paint_dirty = true;
        render(&mut cx);
        let submitted = cx.frame_submission_serial();
        assert!(submitted > last_serial);
        assert_eq!(cx.frame_completion_serial(), submitted);
        // HAL accounts the BGRA8 storage the device gave it: at least the texels (a GPU
        // allocation is rounded up to the driver's alignment; headless counts the texels).
        let bytes = (size * size * 4) as u64;
        let allocated = session.allocation_bytes().expect("the target is allocated");
        assert!(allocated >= bytes, "{allocated} < {bytes}");
        assert_eq!(session.texture_bytes(), baseline + allocated);
        if iteration < 2 && !session.headless {
            let _ticket = texture.read_back(&mut cx, ReadbackRequest::default()).unwrap();
            let result = take_completed(&mut cx).pop().unwrap();
            let (width, height, pixels) = (result.width, result.height, result.data.unwrap());
            assert_eq!((width, height), (size, size));
            assert!(
                unorm_close(&pixels[..4], [64, 128, 191, 255]),
                "InitWith must clear a fresh allocation: {:?}",
                &pixels[..4]
            );
        }
        clone.release(&mut cx);
        assert!(cx.hal_session_turn(0, false));
        assert_eq!(cx.frame_completion_serial(), submitted);
        assert_eq!(texture.texture_id(), id);
        assert_eq!(session.allocation_bytes(), None);
        assert_eq!(session.texture_bytes(), baseline);
        assert_eq!(cx.read_render_texture(&texture, |_| {}), Err(ReadbackError::NotRendered));
        // Idempotent through every clone; format/handle survive.
        texture.release(&mut cx);
        assert_eq!(session.texture_bytes(), baseline);
        last_serial = submitted;
    }
}

#[test]
fn readback_round_trip_512_and_fifty_bounded_tickets() {
    let (mut cx, session) = open_session();
    let texture = Texture::new_with_format(&mut cx, TextureFormat::RenderBGRAu8 {
        size: TextureSize::Fixed { width: 512, height: 512 }, initial: true,
    });
    let pass = DrawPass::new(&mut cx);
    let list = DrawList::new(&mut cx);
    pass.add_color_texture(&mut cx, &texture, DrawPassClearColor::ClearWith(vec4(0.25, 0.5, 0.75, 1.0)));
    pass.set_size(&mut cx, dvec2(512.0, 512.0));
    cx.passes[pass.draw_pass_id()].main_draw_list_id = Some(list.id());
    cx.passes[pass.draw_pass_id()].dpi_factor = Some(1.0);
    cx.passes[pass.draw_pass_id()].paint_dirty = true;
    let first = texture.read_back(&mut cx, ReadbackRequest::default()).unwrap();
    assert!(cx.try_take_texture_readbacks().is_empty());
    render(&mut cx);
    let results = take_completed(&mut cx);
    assert_eq!(results.len(), 1);
    let result = &results[0];
    assert_eq!(result.ticket, first);
    assert_eq!(result.producer_serial, cx.frame_submission_serial());
    assert_eq!((result.width, result.height), (512, 512));
    let generation = result.allocation_generation;
    assert_pixels(result, session.headless, [64, 128, 191, 255]);
    let data = result.data.clone();

    let serial = cx.frame_submission_serial();
    let mut tickets = std::collections::HashSet::new();
    let mut rejected = 0;
    for _ in 0..50 {
        match texture.read_back(&mut cx, ReadbackRequest::default()) {
            Ok(ticket) => { assert!(tickets.insert(ticket)); }
            Err(ReadbackError::Backpressure) => rejected += 1,
            other => panic!("unexpected admission: {other:?}"),
        }
        assert!(cx.texture_readback_usage().reserved_bytes <= TEXTURE_READBACK_MAX_BYTES);
    }
    assert!(rejected > 0, "undrained results must exert backpressure");
    assert_eq!(tickets.len() + rejected, 50);
    // Releasing through another handle cannot invalidate completed/in-flight
    // bytes, and current-content copies do not render or advance the serial.
    texture.clone().release(&mut cx);
    let results = take_completed(&mut cx);
    assert_eq!(results.len(), tickets.len());
    for result in results {
        assert!(tickets.remove(&result.ticket));
        assert_eq!(result.allocation_generation, generation);
        assert_eq!(result.producer_serial, serial);
        assert_eq!(result.data, data);
    }
    assert!(tickets.is_empty());
    assert_eq!(cx.frame_submission_serial(), serial);
    assert_eq!(cx.texture_readback_usage().reserved_bytes, 0);
    assert!(cx.try_take_texture_readbacks().is_empty());

    // Retain rejected work and retry it after draining. Render a distinct
    // byte pattern for each producer, then overwrite/release the target before
    // taking results: each ticket must still identify its own submitted image.
    let mut expected = std::collections::HashMap::new();
    let mut completed = 0;
    for iteration in 0..50u8 {
        let pixel = [iteration, 255 - iteration, iteration * 3, 255];
        cx.passes[pass.draw_pass_id()].color_textures[0].clear_color = DrawPassClearColor::ClearWith(vec4(
            pixel[0] as f32 / 255.0, pixel[1] as f32 / 255.0, pixel[2] as f32 / 255.0, 1.0,
        ));
        cx.passes[pass.draw_pass_id()].paint_dirty = true;
        let ticket = loop {
            match texture.read_back(&mut cx, ReadbackRequest::default()) {
                Ok(ticket) => break ticket,
                Err(ReadbackError::Backpressure) => {
                    let ready = take_completed(&mut cx);
                    assert!(!ready.is_empty());
                    for result in ready {
                        let pixel = expected.remove(&result.ticket).unwrap();
                        assert_pixels(&result, session.headless, pixel);
                        completed += 1;
                    }
                }
                other => panic!("unexpected retry result: {other:?}"),
            }
        };
        assert!(expected.insert(ticket, pixel).is_none());
        render(&mut cx);
        assert!(cx.texture_readback_usage().reserved_bytes <= TEXTURE_READBACK_MAX_BYTES);
    }
    texture.release(&mut cx);
    for result in take_completed(&mut cx) {
        let pixel = expected.remove(&result.ticket).unwrap();
        assert_pixels(&result, session.headless, pixel);
        completed += 1;
    }
    assert!(expected.is_empty());
    assert_eq!(completed, 50);
    assert_eq!(cx.texture_readback_usage().reserved_bytes, 0);
}

#[test]
fn readback_cancellation_and_reallocation_are_terminal() {
    let (mut cx, session) = open_session();
    let texture = Texture::new_with_format(&mut cx, TextureFormat::RenderBGRAu8 {
        size: TextureSize::Fixed { width: 512, height: 512 }, initial: true,
    });
    let pass = DrawPass::new(&mut cx);
    let list = DrawList::new(&mut cx);
    pass.add_color_texture(&mut cx, &texture, DrawPassClearColor::ClearWith(vec4(0.1, 0.2, 0.3, 0.5)));
    pass.set_size(&mut cx, dvec2(512.0, 512.0));
    cx.passes[pass.draw_pass_id()].main_draw_list_id = Some(list.id());
    cx.passes[pass.draw_pass_id()].dpi_factor = Some(1.0);
    cx.passes[pass.draw_pass_id()].paint_dirty = true;
    let cancelled = texture.read_back(&mut cx, ReadbackRequest::default()).unwrap();
    let invalidated = texture.read_back(&mut cx, ReadbackRequest::default()).unwrap();
    assert!(cx.cancel_texture_readback(cancelled));
    texture.release(&mut cx);
    render(&mut cx);
    let results = take_completed(&mut cx);
    assert_eq!(results.len(), 2);
    assert_eq!(results.iter().find(|r| r.ticket == cancelled).unwrap().data, Err(ReadbackError::Cancelled));
    assert_eq!(results.iter().find(|r| r.ticket == invalidated).unwrap().data, Err(ReadbackError::AllocationChanged));
    assert!(!cx.cancel_texture_readback(cancelled));
    let fresh = texture.read_back(&mut cx, ReadbackRequest::default()).unwrap();
    let result = take_completed(&mut cx).pop().unwrap();
    assert_eq!(result.ticket, fresh);
    // Raw attachment channels must not be unpremultiplied by readback.
    assert_pixels(&result, session.headless, [26, 51, 77, 128]);
    let orphan = texture.read_back(&mut cx, ReadbackRequest { next_render: true }).unwrap();
    drop(pass);
    let result = take_completed(&mut cx).pop().unwrap();
    assert_eq!(result.ticket, orphan);
    assert_eq!(result.data, Err(ReadbackError::Cancelled));
}

// The existing renderer tests drive the public native session interface.
struct Session { id: u32, headless: bool }
impl Session {
    fn texture_bytes(&self) -> u64 {
        makepad_hal::Hal::global().report(self.id, 0).unwrap().items.iter()
            .filter(|item| item.kind == makepad_hal::ledger::Kind::Texture)
            .map(|item| item.bytes).sum()
    }
    fn allocation_bytes(&self) -> Option<u64> {
        makepad_hal::Hal::global().report(self.id, 0).unwrap().items.iter()
            .find(|item| item.kind == makepad_hal::ledger::Kind::Texture)
            .map(|item| item.bytes)
    }
}
impl Drop for Session {
    fn drop(&mut self) { makepad_hal::Hal::global().close(self.id); }
}
fn open_session() -> (Cx, Session) {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.init_cx_os();
    let info = makepad_hal::Hal::global().open(&makepad_hal::OpenReq::from_env(
        0, makepad_hal::open_flags::OFFSCREEN_WINDOWS, 1.0));
    cx.hal_attach_session(info);
    (cx, Session { id: info.session, headless: info.backend == makepad_hal::backend_id::HEADLESS })
}
fn render(cx: &mut Cx) {
    assert!(cx.hal_session_turn(0, true));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while cx.frame_completion_serial() < cx.frame_submission_serial() {
        assert!(std::time::Instant::now() < deadline, "HAL frame completion timed out");
        assert!(cx.hal_session_turn(1_000_000, false));
    }
}
fn take_completed(cx: &mut Cx) -> Vec<TextureReadback> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut results = Vec::new();
    while cx.texture_readback_usage().requests != 0 {
        results.extend(cx.try_take_texture_readbacks());
        if cx.texture_readback_usage().requests == 0 { break; }
        assert!(std::time::Instant::now() < deadline, "HAL readback completion timed out");
        assert!(cx.hal_session_turn(1_000_000, false));
    }
    results
}
fn assert_pixels(result: &TextureReadback, headless: bool, pixel: [u8; 4]) {
    if headless {
        assert_eq!(result.data, Err(ReadbackError::UnsupportedBackend));
    } else {
        let data = result.data.as_ref().unwrap();
        assert_eq!(data.len(), result.width * result.height * 4);
        assert!(data.chunks_exact(4).all(|bytes| unorm_close(bytes, pixel)), "{:?} expected {pixel:?}", &data[..4]);
    }
}
/// A float colour written to a UNORM8 target may round either way at .5 (0.5 is 127 or 128): one
/// step per channel is the same colour.
fn unorm_close(bytes: &[u8], pixel: [u8; 4]) -> bool {
    bytes.iter().zip(pixel).all(|(a, b)| a.abs_diff(b) <= 1)
}
