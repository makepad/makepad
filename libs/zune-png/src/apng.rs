/*
 * Copyright (c) 2023.
 *
 * This software is free software;
 *
 * You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */
#![allow(dead_code, unused_imports)] // when building for no_std
use alloc::vec::Vec;

use zune_core::colorspace::ColorSpace;

use crate::error::PngDecodeErrors;
use crate::PngInfo;

/// `num_frames` indicates the total number of frames in the animation. This must equal the number of `fcTL` chunks. 0 is not a valid value.
/// 1 is a valid value for a single-frame APNG.
/// If this value does not equal the actual number of frames it should be treated as an error.
//
// `num_plays` indicates the number of times that this animation should play;
// if it is 0, the animation should play indefinitely.
// If nonzero, the animation should come to rest on the final frame at the end of the last play.
#[derive(Copy, Clone)]
pub struct ActlChunk {
    pub num_frames: u32,
    pub num_plays:  u32
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisposeOp {
    /// No disposal is done on this frame before rendering the next;
    None,
    /// The frame's region of the output buffer is to be
    /// cleared to fully transparent black before rendering the next frame.
    Background,
    /// The frame's region of the output buffer is
    /// to be reverted to the previous contents before rendering the next frame.
    Previous
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlendOp {
    /// all color components of the frame, including alpha,
    /// overwrite the current contents of the frame's output buffer region.
    Source,
    /// Frame should be composited onto the output buffer
    /// based on its alpha, using a simple OVER operation as described
    /// in the "Alpha Channel Processing" section of the PNG specification [PNG-1.2]
    Over
}

impl BlendOp {
    pub fn from_int(int: u8) -> Result<BlendOp, PngDecodeErrors> {
        match int {
            0 => Ok(BlendOp::Source),
            1 => Ok(BlendOp::Over),
            _ => Err(PngDecodeErrors::GenericStatic("Unknown blend operation"))
        }
    }
}

impl DisposeOp {
    pub fn from_int(int: u8) -> Result<DisposeOp, PngDecodeErrors> {
        match int {
            0 => Ok(DisposeOp::None),
            1 => Ok(DisposeOp::Background),
            2 => Ok(DisposeOp::Previous),
            _ => Err(PngDecodeErrors::GenericStatic("Unknown blend operation"))
        }
    }
}

/// Describes a single frame
#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    /// Sequence number of frame. If image isn't an APNG, it is usually
    /// set to zero
    pub seq_number:     i32,
    /// Width of the frame, If image isn't an APNG, it matches the width of
    /// the image
    pub width:          usize,
    /// Height of frame. If image isn't an APNG, it matches the height of the image
    pub height:         usize,
    /// X position at which to render the following frame, if image isn't APNG, set to zero
    pub x_offset:       usize,
    /// Y position at which to render the following frame, if image isn't APNG, set to zero
    pub y_offset:       usize,
    /// Frame delay fraction numerator
    pub delay_num:      u16,
    /// Frame delay fraction denominator
    pub delay_denom:    u16,
    /// Type of frame area disposal to be done after rendering this frame
    pub dispose_op:     DisposeOp,
    /// Type of frame area rendering for this frame
    pub blend_op:       BlendOp,
    /// Whether the frame is supposed to be part of the
    /// animation sequence.
    ///
    /// If an image contains standard IDAT chunks, this is what is to be
    /// displayed in case the decoder doesn't support apng but here
    /// it is usually the first frame
    pub is_part_of_seq: bool
}

/// Represents a single frame
pub struct SingleFrame {
    // can either be idat or fdat, depending
    // on frame number
    pub fdat:      Vec<u8>,
    /// If none, indicates data is IDAT, hence
    /// should be decoded as such
    pub fctl_info: Option<FrameInfo>
}

impl SingleFrame {
    /// Create a new frame
    pub fn new(chunks: Vec<u8>, fctl_info: Option<FrameInfo>) -> SingleFrame {
        SingleFrame {
            fdat: chunks,
            fctl_info
        }
    }
    /// Push a chunk onto this frame
    pub fn push_chunk(&mut self, chunk: &[u8]) {
        self.fdat.extend_from_slice(chunk);
    }
    /// Set Frame control details for this frame
    pub fn set_fctl(&mut self, fctl: FrameInfo) {
        self.fctl_info = Some(fctl);
    }
}

///
/// Convert a single png frame into a full separate image.
///
/// The APNG spec requires post processing of individual frames which can include:
/// - Setting the background to be color,black or the previous frame.
/// - Compositing the current frame with the previous frame.
///
/// This function performs the two operations above given a frame, its details and an optional previous frame
///
///
/// In case of alpha compositing, we do the blending in linear space. The code we use matches what is
/// specified by the spec at [Alpha Channel Processing](https://www.w3.org/TR/2003/REC-PNG-20031110/#13Alpha-channel-processing)
///
///
///  # Requires
/// Requires the `#[std]` feature as we need `powf` function from the standard library
/// # Arguments
///
/// * `info`: Png information, this should contain width and height of the main image
/// * `colorspace`: The image colorspace, this can be obtained from a decoder `get_colorspace()`
/// * `frame_info`: The current frame information , for the current frame, it is obtained by [.frame_info()](crate::PngDecoder::frame_info)
/// * `current_frame`: This is the current frame which we are processing
/// * `prev_frame`: An optional previous frame. In case the frame information says we use
///  the previous frame as background this frame will be copied to the output before blending the image
///   This is the fully processed previous frame, hence the width and height should match that of the image (indicated by `info.width`,`info.height`)
/// * `output`: The output of the processed frame. The dimensions of this should be (`info.width * info.height*decoder.colorspace().num_components()`)
/// * `gamma`: An optional gamma value. It is best this is retrieved from [the image's gamma](crate::PngInfo.gamma),
/// if `None`, we will default to use 2.2 (default gamma value)
///
/// returns: Ok(()) Output is written to `output` variable
///
/// # Examples
///
/// ```no_run
/// use zune_core::options::EncoderOptions;
/// use zune_png::{PngDecoder, post_process_image};
/// // read the file
/// // set up decoder
/// let mut decoder = PngDecoder::new(&[]);
/// // decode headers
/// decoder.decode_headers().unwrap();
/// // get useful information about the image
/// let colorspace = decoder.get_colorspace().unwrap();
/// let depth = decoder.get_depth().unwrap();
/// //  get decoder information,we clone this because we need a standalone
/// // info since we mutably modify decoder struct below
/// let info = decoder.get_info().unwrap().clone();
/// // set up our background variable. Soon it will contain the data for the previous
/// // frame, the first frame has no background hence why this is None
/// let mut background: Option<Vec<u8>> = None;
/// // the output, since we know that no frame will be bigger than the width and height, we can
/// // set this up outside of the loop.
/// let mut output =
///  vec![0; info.width * info.height * decoder.get_colorspace().unwrap().num_components()];
/// let mut i = 0;
///
/// while decoder.more_frames() {
///     // decode the header, in case we haven't processed a frame header
///    decoder.decode_headers().unwrap();
///     // then decode the current frame information,
///     // NB: Frame information is for current frame hence should be accessed before decoding the frame
///     // as it will change on subsequent frames
///    let frame = decoder.frame_info().unwrap();
///    // decode the raw pixels, even on smaller frames, we only allocate frame_info.width*frame_info.height
///    let pix = decoder.decode_raw().unwrap();
///     // call post process
///    post_process_image(
///             &info,
///             colorspace,
///             &frame,
///             &pix,
///             background.as_ref().map(|x| x.as_slice()),
///             &mut output,
///             None
///    ).unwrap();
///     // create encoder parameters
///    let encoder_opts = EncoderOptions::new(info.width, info.height, colorspace, depth);
///
///    let bytes = zune_png::PngEncoder::new(&output, encoder_opts).encode();
///
///     //std::fs::write(format!("./{i}.png"), bytes).unwrap();
///     // this is expensive, but we need a copy of the previous fully rendered frame
///     // we can alleviate this since we are using the same output, so DisposeOP::None will always be the
///     // same as DisposeOp::Previous, but only works for this example.
///     // in case you reuse the same buffer per invocation,
///     // always have your background as None
///     background = Some(output.clone());
///    i += 1;
/// }
/// ```
#[cfg(feature = "std")]
pub fn post_process_image(
    info: &PngInfo, colorspace: ColorSpace, frame_info: &FrameInfo, current_frame: &[u8],
    prev_frame: Option<&[u8]>, output: &mut [u8], gamma: Option<f32>
) -> Result<(), PngDecodeErrors> {
    let nc = colorspace.num_components();
    //
    // check invariants
    if frame_info.x_offset + frame_info.width > info.width {
        return Err(PngDecodeErrors::GenericStatic(
            "Frame X offset + frame width larger than image width"
        ));
    }
    if frame_info.y_offset + frame_info.height > info.height {
        return Err(PngDecodeErrors::GenericStatic(
            "Frame y offset + frame height larger than image height"
        ));
    }
    // current frame matches the image frame
    let frame_dims = frame_info.height * frame_info.width * nc;

    // ensure we can have at least enough space to write output
    if current_frame.len() < frame_dims {
        let msg = format!(
            "Current frame dimensions ({}) less than  expected dimensions ({})",
            current_frame.len(),
            frame_dims
        );
        return Err(PngDecodeErrors::Generic(msg));
    }

    match frame_info.dispose_op {
        DisposeOp::None => {} // do nothing
        DisposeOp::Background => {
            // output to fully black
            output.fill(0);
        }
        DisposeOp::Previous => {
            // copy background if possible
            if let Some(data) = prev_frame {
                if output.len() != data.len() {
                    return Err(PngDecodeErrors::GenericStatic(
                        "Previous frame does not match output length"
                    ));
                }
                output.copy_from_slice(data);
            } else if frame_info.seq_number > 1 {
                return Err(PngDecodeErrors::GenericStatic(
                    "A frame whose DisposeOp is Previous did not get a previous frame"
                ));
            }
        } // blend
    }
    // move to the start of the y position to render frame
    let start_pos = frame_info.y_offset * info.width * nc;

    let start = &mut output[start_pos..];
    // deal with gamma
    let gamma_value = gamma.unwrap_or(2.2);
    let gamma_inv = 1.0 / gamma_value;

    // iterate by a width stride
    for (src_width, h) in current_frame.chunks_exact(frame_info.width * nc).zip(
        start
            .chunks_exact_mut(info.width * nc)
            .take(frame_info.height)
    ) {
        // move to the x offset
        let h_x = &mut h[frame_info.x_offset * nc..(frame_info.x_offset + frame_info.width) * nc];

        // now blend
        match frame_info.blend_op {
            BlendOp::Source => {
                // overwrite
                h_x.copy_from_slice(src_width);
            }
            BlendOp::Over => {
                // carry out alpha blending, we do the alpha bending in linear colorspace
                //
                assert!(colorspace.has_alpha());
                if !colorspace.has_alpha() {
                    return Err(PngDecodeErrors::GenericStatic("Image doesn't have alpha but requests blending using over which requires alpha"));
                }
                // pre-calculate the gamma samples
                let mut gamma_values = [0.0; 256];
                let max_sample = 255.0;

                for (i, item) in gamma_values.iter_mut().enumerate() {
                    let gam = (i as f32) / max_sample;
                    let linfg = f32::powf(gam, gamma_inv);
                    *item = linfg;
                }

                for (src_comp, dst_comp) in src_width.chunks_exact(nc).zip(h.chunks_exact_mut(nc)) {
                    let foreground_alpha = f32::from(*src_comp.last().unwrap()) / 255.0;
                    let dst_alpha = 1.0 - foreground_alpha;

                    let max_sample = 255.0;

                    for (a, b) in src_comp.iter().zip(dst_comp).take(nc - 1) {
                        // convert to floating point, undo gamma encoding
                        let linfg = gamma_values[usize::from(*a)];
                        let linbg = gamma_values[usize::from(*b)];
                        // composite
                        let commpix = linfg * foreground_alpha + linbg * dst_alpha;
                        // scale up and output to b
                        *b = (commpix * max_sample + 0.5) as u8;
                    }
                }
            }
        }
    }
    Ok(())
}
