//! Run with MAKEPAD=headless RUSTFLAGS='--cfg headless' cargo test --release
//! -p makepad-platform --test texture_lifetime. No window, capture or JIT is
//! needed: an empty draw list exercises the real render-pass clear/allocation.
#![cfg(headless)]
use makepad_platform::*;

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
        cx.headless_render_all_passes(iteration as f64);
        let submitted = cx.frame_submission_serial();
        assert!(submitted > last_serial);
        assert_eq!(cx.frame_completion_serial(), submitted);
        // Headless stores RGBA float pixels even for a BGRA8 target.
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
