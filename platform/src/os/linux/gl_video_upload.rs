//! Shared GL upload helpers for YUV plane textures.
//! Used by both desktop Linux (linux_video_player) and Android.

use {
    super::gl_sys::{self, LibGl},
    crate::{
        texture::{CxTexturePool, TextureAlloc, TextureCategory, TextureId, TexturePixel},
        video_decode::yuv::YuvPlaneData,
    },
    std::ffi::c_void,
};

pub(crate) fn upload_yuv_to_gl(
    gl: &LibGl,
    textures: &mut CxTexturePool,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    tex_v_id: TextureId,
    planes: &YuvPlaneData,
) {
    let (cw, ch) = planes.layout.chroma_size(planes.width, planes.height);
    upload_r8_plane_to_gl(
        gl,
        textures,
        tex_y_id,
        &planes.y,
        planes.width,
        planes.height,
        planes.y_bytes_per_row(),
    );
    upload_r8_plane_to_gl(
        gl,
        textures,
        tex_u_id,
        &planes.u,
        cw,
        ch,
        planes.u_bytes_per_row(),
    );
    upload_r8_plane_to_gl(
        gl,
        textures,
        tex_v_id,
        &planes.v,
        cw,
        ch,
        planes.v_bytes_per_row(),
    );
}

pub(crate) fn upload_i420_slices_to_gl(
    gl: &LibGl,
    textures: &mut CxTexturePool,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    tex_v_id: TextureId,
    y: &[u8],
    u: &[u8],
    v: &[u8],
    width: u32,
    height: u32,
) {
    let cw = width.div_ceil(2);
    upload_i420_planes_to_gl(
        gl, textures, tex_y_id, tex_u_id, tex_v_id, y, u, v, width, height, width, cw, cw,
    );
}

pub(crate) fn upload_i420_planes_to_gl(
    gl: &LibGl,
    textures: &mut CxTexturePool,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    tex_v_id: TextureId,
    y: &[u8],
    u: &[u8],
    v: &[u8],
    width: u32,
    height: u32,
    y_stride: u32,
    u_stride: u32,
    v_stride: u32,
) {
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    upload_r8_plane_to_gl(gl, textures, tex_y_id, y, width, height, y_stride);
    upload_r8_plane_to_gl(gl, textures, tex_u_id, u, cw, ch, u_stride);
    upload_r8_plane_to_gl(gl, textures, tex_v_id, v, cw, ch, v_stride);
}

pub(crate) fn upload_r8_plane_to_gl(
    gl: &LibGl,
    textures: &mut CxTexturePool,
    texture_id: TextureId,
    data: &[u8],
    width: u32,
    height: u32,
    bytes_per_row: u32,
) {
    let w = width as usize;
    let h = height as usize;
    let bpr = bytes_per_row.max(width) as usize;
    if w == 0 || h == 0 || data.len() < bpr * (h.saturating_sub(1)) + w {
        return;
    }

    unsafe {
        let cxtexture = &mut textures[texture_id];
        let needs_alloc = if cxtexture.os.gl_texture.is_none() {
            let mut gl_texture = std::mem::MaybeUninit::uninit();
            (gl.glGenTextures)(1, gl_texture.as_mut_ptr());
            let gl_texture = gl_texture.assume_init();
            cxtexture.os.gl_texture = Some(gl_texture);

            (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);
            (gl.glTexParameteri)(
                gl_sys::TEXTURE_2D,
                gl_sys::TEXTURE_WRAP_S,
                gl_sys::CLAMP_TO_EDGE as i32,
            );
            (gl.glTexParameteri)(
                gl_sys::TEXTURE_2D,
                gl_sys::TEXTURE_WRAP_T,
                gl_sys::CLAMP_TO_EDGE as i32,
            );
            (gl.glTexParameteri)(
                gl_sys::TEXTURE_2D,
                gl_sys::TEXTURE_MIN_FILTER,
                gl_sys::LINEAR as i32,
            );
            (gl.glTexParameteri)(
                gl_sys::TEXTURE_2D,
                gl_sys::TEXTURE_MAG_FILTER,
                gl_sys::LINEAR as i32,
            );
            true
        } else {
            cxtexture
                .alloc
                .as_ref()
                .map_or(true, |a| a.width != w || a.height != h)
        };

        let gl_texture = cxtexture.os.gl_texture.unwrap();
        (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);
        (gl.glPixelStorei)(gl_sys::UNPACK_ALIGNMENT, 1);
        (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, bpr as i32);
        (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_PIXELS, 0);
        (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_ROWS, 0);

        if needs_alloc {
            (gl.glTexImage2D)(
                gl_sys::TEXTURE_2D,
                0,
                gl_sys::R8 as i32,
                w as i32,
                h as i32,
                0,
                gl_sys::RED,
                gl_sys::UNSIGNED_BYTE,
                data.as_ptr() as *const c_void,
            );
        } else {
            (gl.glTexSubImage2D)(
                gl_sys::TEXTURE_2D,
                0,
                0,
                0,
                w as i32,
                h as i32,
                gl_sys::RED,
                gl_sys::UNSIGNED_BYTE,
                data.as_ptr() as *const c_void,
            );
        }

        // Restore default unpack state so other uploads stay packed.
        (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, 0);
        (gl.glBindTexture)(gl_sys::TEXTURE_2D, 0);

        cxtexture.alloc = Some(TextureAlloc {
            width: w,
            height: h,
            pixel: TexturePixel::VideoYuvPlane,
            category: TextureCategory::Video,
        });
    }
}
