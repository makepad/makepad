/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

use alloc::vec::Vec;

use makepad_zune_core::bytestream::{ZByteIoError, ZByteWriterTrait, ZWriter};
use makepad_zune_core::colorspace::ColorSpace;

use crate::crc::{calc_crc, calc_crc_with_bytes};
use crate::decoder::PngChunk;
use crate::encoder::PngEncoder;

pub(crate) fn write_ihdr(ctx: &PngEncoder, output: &mut ZWriter<&mut Vec<u8>>) {
    // write width and height
    output.write_u32_be(ctx.options.width() as u32);
    output.write_u32_be(ctx.options.height() as u32);
    // write depth
    output.write_u8(ctx.options.depth().bit_size() as u8);
    // write color
    let color = ctx.options.colorspace();

    let color_int = match color {
        ColorSpace::Luma => 0,
        ColorSpace::RGB => 2,
        ColorSpace::LumaA => 4,
        ColorSpace::RGBA => 6,
        _ => unreachable!(),
    };
    output.write_u8(color_int);
    //compression method
    output.write_u8(0);
    // filter method for first row
    output.write_u8(ctx.row_filter.to_int());
    // interlace method, always Standard
    output.write_u8(0);
}

pub fn write_exif(ctx: &PngEncoder, writer: &mut ZWriter<&mut alloc::vec::Vec<u8>>) {
    if let Some(exif) = ctx.exif {
        writer.write_all(exif).unwrap();
    }
}

pub fn write_gamma(ctx: &PngEncoder, writer: &mut ZWriter<&mut Vec<u8>>) {
    if let Some(gamma) = ctx.gamma {
        // scale by 100000.0
        let gamma_value = (gamma * 100000.0) as u32;
        writer.write_u32_be(gamma_value);
    }
}

// iend is a no-op
pub fn write_iend(_: &PngEncoder, _: &mut ZWriter<&mut Vec<u8>>) {}

/// Write header writes the boilerplate for each png chunk
///
/// It writes the length, chunk type, calls a function to write the
/// data and then calculates the CRC chunk for that png and writes it.
///
/// This should be called with the appropriate inner function to write data
///
pub fn write_header_fn<T: ZByteWriterTrait, F: Fn(&PngEncoder, &mut ZWriter<&mut Vec<u8>>)>(
    v: &PngEncoder,
    writer: &mut ZWriter<T>,
    name: &[u8; 4],
    func: F,
) -> Result<(), ZByteIoError> {
    // We use a vec so that we make crc calculations easier for myself
    // and the problem is that how png chunks work is that you have to go back and write length
    // but you can't know the length without writing the whole thing,
    //

    // format
    // length - chunk type - [data] -  crc chunk
    let mut temp_space = Vec::with_capacity(10);
    // space for length
    temp_space.extend_from_slice(&[0; 4]);
    let mut local_writer = ZWriter::new(&mut temp_space);
    // write the type
    local_writer.write_all(name).unwrap();
    // call underlying function
    (func)(v, &mut local_writer);
    // get bytes written;
    let bytes_written = local_writer.bytes_written();
    // write length less the chunk name
    temp_space[0..4].copy_from_slice(&(bytes_written as u32 - 4).to_be_bytes());
    // write crc, ignore the length
    let c = calc_crc(&temp_space[4..]);
    temp_space.extend_from_slice(&c.to_be_bytes());

    writer.write_all(&temp_space)
}

pub(crate) fn write_chunk<T: ZByteWriterTrait>(
    chunk: PngChunk,
    data: &[u8],
    writer: &mut ZWriter<T>,
) -> Result<(), ZByteIoError> {
    // write length
    writer.write_u32_be_err(chunk.length as u32)?;
    // // write chunk name
    writer.write_all(&chunk.chunk)?;
    // // write chunk data
    writer.write_all(data)?;
    // crc is a continuous function, so first crc the chunk name
    // and then crc that with the chunk bytes passing in the previous crc

    // equal to crc((chunk.chunk + data) ,u32::MAX))
    let crc = calc_crc_with_bytes(&chunk.chunk, u32::MAX);
    let crc = !calc_crc_with_bytes(data, crc);
    writer.write_u32_be_err(crc)?;
    Ok(())
}
