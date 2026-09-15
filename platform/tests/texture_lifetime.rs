//! Run with CARGO_TARGET_DIR=target-gpusim MAKEPAD=gpusim RUSTFLAGS='--cfg gpusim' cargo test --release
//! -p makepad-platform --test texture_lifetime. No window, capture or JIT is
//! needed: an empty draw list exercises the real render-pass clear/allocation.
#![cfg(gpusim)]
use makepad_platform::*;
use makepad_platform::os::{ReadbackChannelOrder, ReadbackError, ReadbackOrigin, ReadbackRequest, TEXTURE_READBACK_MAX_BYTES};

#[test]
fn released_render_texture_reallocates_fifty_sizes_without_retaining_storage() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
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
    let baseline = cx.texture_pool_bytes();
    assert_eq!(texture.allocated_bytes(&cx), None);
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
        cx.gpusim_render_all_passes(iteration as f64);
        let submitted = cx.frame_submission_serial();
        assert!(submitted > last_serial);
        assert_eq!(cx.frame_completion_serial(), submitted);
        // Gpusim stores RGBA float pixels even for a BGRA8 target.
        let bytes = (size * size * 16) as u64;
        assert_eq!(texture.allocated_bytes(&cx), Some(bytes));
        assert_eq!(cx.texture_pool_bytes(), baseline + bytes);
        if iteration < 2 {
            let (width, height, pixels) = cx.debug_read_render_texture(&texture).unwrap();
            assert_eq!((width, height), (size, size));
            assert_eq!(
                &pixels[..4],
                &[191, 128, 64, 255],
                "InitWith must clear a fresh allocation"
            );
        }
        clone.release(&mut cx);
        assert_eq!(cx.frame_completion_serial(), submitted);
        assert_eq!(texture.texture_id(), id);
        assert_eq!(texture.allocated_bytes(&cx), None);
        assert_eq!(cx.texture_pool_bytes(), baseline);
        assert!(cx.debug_read_render_texture(&texture).is_none());
        // Idempotent through every clone; format/handle survive.
        texture.release(&mut cx);
        assert_eq!(cx.texture_pool_bytes(), baseline);
        last_serial = submitted;
    }
}

#[test]
fn readback_round_trip_512_and_fifty_bounded_tickets() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
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
    cx.gpusim_render_all_passes(0.0);
    let results = cx.try_take_texture_readbacks();
    assert_eq!(results.len(), 1);
    let result = &results[0];
    assert_eq!(result.ticket, first);
    assert_eq!(result.producer_serial, cx.frame_submission_serial());
    assert_eq!((result.width, result.height, result.stride), (512, 512, 2048));
    assert_eq!(result.channel_order, ReadbackChannelOrder::Bgra);
    assert_eq!(result.origin, ReadbackOrigin::TopLeft);
    let generation = result.allocation_generation;
    let data = result.data.as_ref().unwrap();
    assert_eq!(data.len(), 512 * 512 * 4);
    assert!(data.chunks_exact(4).all(|pixel| pixel == [191, 128, 64, 255]));

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
    let results = cx.try_take_texture_readbacks();
    assert_eq!(results.len(), tickets.len());
    for result in results {
        assert!(tickets.remove(&result.ticket));
        assert_eq!(result.allocation_generation, generation);
        assert_eq!(result.producer_serial, serial);
        assert_eq!(result.data.unwrap().as_ref(), data.as_ref());
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
            pixel[2] as f32 / 255.0, pixel[1] as f32 / 255.0, pixel[0] as f32 / 255.0, 1.0,
        ));
        cx.passes[pass.draw_pass_id()].paint_dirty = true;
        let ticket = loop {
            match texture.read_back(&mut cx, ReadbackRequest::default()) {
                Ok(ticket) => break ticket,
                Err(ReadbackError::Backpressure) => {
                    let ready = cx.try_take_texture_readbacks();
                    assert!(!ready.is_empty());
                    for result in ready {
                        let pixel = expected.remove(&result.ticket).unwrap();
                        assert!(result.data.unwrap().chunks_exact(4).all(|bytes| bytes == pixel));
                        completed += 1;
                    }
                }
                other => panic!("unexpected retry result: {other:?}"),
            }
        };
        assert!(expected.insert(ticket, pixel).is_none());
        cx.gpusim_render_all_passes(iteration as f64 + 1.0);
        assert!(cx.texture_readback_usage().reserved_bytes <= TEXTURE_READBACK_MAX_BYTES);
    }
    texture.release(&mut cx);
    for result in cx.try_take_texture_readbacks() {
        let pixel = expected.remove(&result.ticket).unwrap();
        assert!(result.data.unwrap().chunks_exact(4).all(|bytes| bytes == pixel));
        completed += 1;
    }
    assert!(expected.is_empty());
    assert_eq!(completed, 50);
    assert_eq!(cx.texture_readback_usage().reserved_bytes, 0);
}

#[test]
fn readback_cancellation_and_reallocation_are_terminal() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
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
    cx.gpusim_render_all_passes(0.0);
    let results = cx.try_take_texture_readbacks();
    assert_eq!(results.len(), 2);
    assert_eq!(results.iter().find(|r| r.ticket == cancelled).unwrap().data, Err(ReadbackError::Cancelled));
    assert_eq!(results.iter().find(|r| r.ticket == invalidated).unwrap().data, Err(ReadbackError::AllocationChanged));
    assert!(!cx.cancel_texture_readback(cancelled));
    let fresh = texture.read_back(&mut cx, ReadbackRequest::default()).unwrap();
    let result = cx.try_take_texture_readbacks().pop().unwrap();
    assert_eq!(result.ticket, fresh);
    // Raw attachment channels must not be unpremultiplied by readback.
    assert_eq!(&result.data.unwrap()[..4], &[77, 51, 26, 128]);
    let orphan = texture.read_back(&mut cx, ReadbackRequest { next_render: true }).unwrap();
    drop(pass);
    let result = cx.try_take_texture_readbacks().pop().unwrap();
    assert_eq!(result.ticket, orphan);
    assert_eq!(result.data, Err(ReadbackError::Cancelled));
}
