/*
 * Copyright (c) 2023.
 *
 * This software is free software;
 *
 * You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

use alloc::vec::Vec;

use crate::error::PngDecodeErrors;

/// `num_frames` indicates the total number of frames in the animation. This must equal the number of `fcTL` chunks. 0 is not a valid value.
/// 1 is a valid value for a single-frame APNG.
/// If this value does not equal the actual number of frames it should be treated as an error.
//
// `num_plays` indicates the number of times that this animation should play;
// if it is 0, the animation should play indefinitely.
// If nonzero, the animation should come to rest on the final frame at the end of the last play.
pub struct ActlChunk {
    pub num_frames: u32,
    pub num_plays:  u32
}

#[derive(Clone, Copy)]
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

#[derive(Clone, Copy)]
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
#[derive(Clone, Copy)]
pub struct FrameInfo {
    pub seq_number:  u32,
    pub width:       usize,
    pub height:      usize,
    pub x_offset:    usize,
    pub y_offset:    usize,
    pub delay_num:   u16,
    pub delay_denom: u16,
    pub dispose_op:  DisposeOp,
    pub blend_op:    BlendOp
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
