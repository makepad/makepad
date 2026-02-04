// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use crate::render::Context;

pub fn apply(
    clip: &usvg::ClipPath,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::Pixmap,
) {
    let mut clip_pixmap = tiny_skia::Pixmap::new(pixmap.width(), pixmap.height()).unwrap();
    clip_pixmap.fill(tiny_skia::Color::BLACK);

    draw_children(
        clip.root(),
        tiny_skia::BlendMode::Clear,
        transform.pre_concat(clip.transform()),
        &mut clip_pixmap.as_mut(),
    );

    if let Some(clip) = clip.clip_path() {
        apply(clip, transform, pixmap);
    }

    let mut mask = tiny_skia::Mask::from_pixmap(clip_pixmap.as_ref(), tiny_skia::MaskType::Alpha);
    mask.invert();
    pixmap.apply_mask(&mask);
}

fn draw_children(
    parent: &usvg::Group,
    mode: tiny_skia::BlendMode,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) {
    for child in parent.children() {
        match child {
            usvg::Node::Path(ref path) => {
                if !path.is_visible() {
                    continue;
                }

                // We could use any values here. They will not be used anyway.
                let ctx = Context {
                    max_bbox: tiny_skia::IntRect::from_xywh(0, 0, 1, 1).unwrap(),
                };

                crate::path::fill_path(path, mode, &ctx, transform, pixmap);
            }
            usvg::Node::Text(ref text) => {
                draw_children(text.flattened(), mode, transform, pixmap);
            }
            usvg::Node::Group(ref group) => {
                let transform = transform.pre_concat(group.transform());

                if let Some(clip) = group.clip_path() {
                    // If a `clipPath` child also has a `clip-path`
                    // then we should render this child on a new canvas,
                    // clip it, and only then draw it to the `clipPath`.
                    clip_group(group, clip, transform, pixmap);
                } else {
                    draw_children(group, mode, transform, pixmap);
                }
            }
            _ => {}
        }
    }
}

fn clip_group(
    children: &usvg::Group,
    clip: &usvg::ClipPath,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) -> Option<()> {
    let mut clip_pixmap = tiny_skia::Pixmap::new(pixmap.width(), pixmap.height()).unwrap();

    draw_children(
        children,
        tiny_skia::BlendMode::SourceOver,
        transform,
        &mut clip_pixmap.as_mut(),
    );
    apply(clip, transform, &mut clip_pixmap);

    let mut paint = tiny_skia::PixmapPaint::default();
    paint.blend_mode = tiny_skia::BlendMode::Xor;
    pixmap.draw_pixmap(
        0,
        0,
        clip_pixmap.as_ref(),
        &paint,
        tiny_skia::Transform::identity(),
        None,
    );

    Some(())
}
