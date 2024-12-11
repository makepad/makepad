/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

use zune_core::bytestream::ZByteWriter;
use zune_core::colorspace::ColorSpace;

use crate::crc::calc_crc;
use crate::decoder::PngChunk;
use crate::encoder::PngEncoder;

pub(crate) fn write_ihdr(ctx: &PngEncoder, output: &mut ZByteWriter) {
    // write width and height
    output.write_u32_be(ctx.options.get_width() as u32);
    output.write_u32_be(ctx.options.get_height() as u32);
    // write depth
    output.write_u8(ctx.options.get_depth().bit_size() as u8);
    // write color
    let color = ctx.options.get_colorspace();

    let color_int = match color {
        ColorSpace::Luma => 0,
        ColorSpace::RGB => 2,
        ColorSpace::LumaA => 4,
        ColorSpace::RGBA => 6,
        _ => unreachable!()
    };
    output.write_u8(color_int);
    //compression method
    output.write_u8(0);
    // filter method for first row
    output.write_u8(ctx.row_filter.to_int());
    // interlace method, always Standard
    output.write_u8(0);
}

pub fn write_exif(ctx: &PngEncoder, writer: &mut ZByteWriter) {
    if let Some(exif) = ctx.exif {
        writer.write_all(exif).unwrap();
    }
}

pub fn write_gamma(ctx: &PngEncoder, writer: &mut ZByteWriter) {
    if let Some(gamma) = ctx.gamma {
        // scale by 100000.0
        let gamma_value = (gamma * 100000.0) as u32;
        writer.write_u32_be(gamma_value);
    }
}

// iend is a no-op
pub fn write_iend(_: &PngEncoder, _: &mut ZByteWriter) {}

/// Write header writes the boilerplate for each png chunk
///
/// It writes the length, chunk type, calls a function to write the
/// data and then calculates the CRC chunk for that png and writes it.
///
/// This should be called with the appropriate inner function to write data
///
pub fn write_header_fn<F: Fn(&PngEncoder, &mut ZByteWriter)>(
    v: &PngEncoder, writer: &mut ZByteWriter, name: &[u8; 4], func: F
) {
    // format
    // length - chunk type - [data] -  crc chunk
    // add space for length
    writer.skip(4);
    // points to chunk type -> going forward
    let start = writer.position();

    // write type
    writer.write_all(name).unwrap();
    // call the underlying function
    (func)(v, writer);
    // get end
    let end = writer.position();

    // skip to start and write length
    let length_start = start - 4;
    writer.set_position(length_start);
    let length = end - start;
    // length does not include the chunk type, so
    // subtract 4
    writer.write_u32_be((length - 4) as u32);

    // go back to end and write hash
    writer.set_position(start);

    let bytes = writer.peek_at(0, length).unwrap();
    let crc32 = calc_crc(bytes);

    writer.set_position(end);
    writer.write_u32_be(crc32);
}

pub(crate) fn write_chunk(chunk: PngChunk, data: &[u8], writer: &mut ZByteWriter) {
    // write length
    writer.write_u32_be(chunk.length as u32);
    // points to chunk type+data
    let start_chunk = writer.position();
    // write chunk name
    writer.write_all(&chunk.chunk).unwrap();
    // write chunk data
    writer.write_all(data).unwrap();
    let end = writer.position();
    // write crc
    // go back to where start chunk points to
    writer.set_position(start_chunk);
    // get everything until where we wrote

    let data = writer.peek_at(0, 4/*name*/ + data.len()).unwrap();
    let crc32 = calc_crc(data);
    // go back to bytes past data
    writer.set_position(end);
    // and write crc32
    writer.write_u32_be(crc32);
}
