use crate::util::*;
use crate::{ImageResult, ImageSize};

use std::io::{BufRead, Cursor, Seek, SeekFrom};

pub fn size<R: BufRead + Seek>(reader: &mut R) -> ImageResult<ImageSize> {
    reader.seek(SeekFrom::Start(0))?;

    let mut endian_marker = [0; 2];
    reader.read_exact(&mut endian_marker)?;

    //  Get the endianness which determines how we read the input
    let endianness = if &endian_marker[0..2] == b"II" {
        Endian::Little
    } else if &endian_marker[0..2] == b"MM" {
        Endian::Big
    } else {
        //  Shouldn't get here by normal means, but handle invalid header anyway
        return Err(
            std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid TIFF header").into(),
        );
    };

    //  Read the IFD offset from the header
    reader.seek(SeekFrom::Start(4))?;
    let ifd_offset = read_u32(reader, &endianness)?;

    //  IFD offset cannot be 0
    if ifd_offset == 0 {
        return Err(
            std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid IFD offset").into(),
        );
    }

    //  Jump to the IFD offset
    reader.seek(SeekFrom::Start(ifd_offset.into()))?;

    //  Read how many IFD records there are
    let ifd_count = read_u16(reader, &endianness)?;
    let mut width = None;
    let mut height = None;

    for _ifd in 0..ifd_count {
        let tag = read_u16(reader, &endianness)?;
        let kind = read_u16(reader, &endianness)?;
        let _count = read_u32(reader, &endianness)?;

        let value_bytes = match kind {
            // BYTE | ASCII | SBYTE | UNDEFINED
            1 | 2 | 6 | 7 => 1,
            // SHORT | SSHORT
            3 | 8 => 2,
            // LONG | SLONG | FLOAT | IFD
            4 | 9 | 11 | 13 => 4,
            // RATIONAL | SRATIONAL
            5 | 10 => 4 * 2,
            // DOUBLE | LONG8 | SLONG8 | IFD8
            12 | 16 | 17 | 18 => 8,
            // Anything else is invalid
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Invalid IFD type",
                )
                .into())
            }
        };

        let mut value_buffer = [0; 4];
        reader.read_exact(&mut value_buffer)?;

        let mut r = Cursor::new(&value_buffer[..]);
        let value = match value_bytes {
            2 => Some(read_u16(&mut r, &endianness)? as u32),
            4 => Some(read_u32(&mut r, &endianness)?),
            _ => None,
        };

        //  Tag 0x100 is the image width, 0x101 is image height
        if tag == 0x100 {
            width = value;
        } else if tag == 0x101 {
            height = value;
        }

        //  If we've read both values we need, return the data
        if let (Some(width), Some(height)) = (width, height) {
            return Ok(ImageSize {
                width: width as usize,
                height: height as usize,
            });
        }
    }

    //  If no width/height pair was found return invalid data
    Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "No dimensions in IFD tags").into())
}

pub fn matches(header: &[u8]) -> bool {
    header.starts_with(b"II\x2A\x00") || header.starts_with(b"MM\x00\x2A")
}
