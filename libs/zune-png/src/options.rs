/*
 * Copyright (c) 2023.
 *
 * This software is free software;
 *
 * You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

use alloc::format;

use zune_core::bytestream::{ZByteReader, ZReaderTrait};
use zune_core::log::trace;

use crate::error::PngDecodeErrors;

pub fn default_chunk_handler<T>(
    length: usize, chunk_type: [u8; 4], reader: &mut ZByteReader<T>, _crc: u32
) -> Result<(), PngDecodeErrors>
where
    T: ZReaderTrait
{
    let chunk_name = core::str::from_utf8(&chunk_type).unwrap_or("XXXX");

    if chunk_type[0] & (1 << 5) == 0 {
        return Err(PngDecodeErrors::Generic(format!(
            "Marker {chunk_name} unknown but deemed necessary",
        )));
    }

    trace!("Encountered unknown chunk {:?}", chunk_name);
    trace!("Length of chunk {}", length);
    trace!("Skipping {} bytes", length + 4);

    reader.skip(length + 4);

    Ok(())
}
