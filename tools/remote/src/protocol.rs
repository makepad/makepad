//! Wire protocol shared by the makepad-remote server, its CLI client and
//! the mapfleet dispatcher. Length-prefixed tagged frames over TCP.

use std::io::{self, Read, Write};

// Client -> server request tags.
pub const TAG_FILE_DATA: u8 = 0x01;
pub const TAG_CARGO_RUN: u8 = 0x02;
pub const TAG_SHELL_RUN: u8 = 0x03;
/// Request one file back from the server's working directory: payload is
/// the relative path. Server answers TAG_FILE_DATA (same encoding as the
/// push direction) then TAG_EXIT_CODE 0, or TAG_ERROR.
pub const TAG_FILE_PULL: u8 = 0x04;

// Server -> client response tags.
pub const TAG_OUTPUT: u8 = 0x01;
pub const TAG_EXIT_CODE: u8 = 0x02;
pub const TAG_ERROR: u8 = 0x03;

pub const STREAM_STDOUT: u8 = 1;
pub const STREAM_STDERR: u8 = 2;

pub fn write_u32(w: &mut dyn Write, v: u32) -> io::Result<()> {
    w.write_all(&v.to_be_bytes())
}

pub fn read_u32(r: &mut dyn Read) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_be_bytes(buf))
}

pub fn write_msg(w: &mut dyn Write, tag: u8, payload: &[u8]) -> io::Result<()> {
    w.write_all(&[tag])?;
    write_u32(w, payload.len() as u32)?;
    w.write_all(payload)?;
    w.flush()
}

pub fn read_msg(r: &mut dyn Read) -> io::Result<(u8, Vec<u8>)> {
    let mut tag_buf = [0u8; 1];
    r.read_exact(&mut tag_buf)?;
    let len = read_u32(r)? as usize;
    let mut payload = vec![0u8; len];
    if len > 0 {
        r.read_exact(&mut payload)?;
    }
    Ok((tag_buf[0], payload))
}

pub fn encode_file_data(rel_path: &str, data: &[u8]) -> Vec<u8> {
    let path_bytes = rel_path.as_bytes();
    let mut buf = Vec::with_capacity(4 + path_bytes.len() + data.len());
    buf.extend_from_slice(&(path_bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(path_bytes);
    buf.extend_from_slice(data);
    buf
}

pub fn decode_file_data(payload: &[u8]) -> io::Result<(&str, &[u8])> {
    if payload.len() < 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file data too short",
        ));
    }
    let path_len = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    if payload.len() < 4 + path_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file data truncated",
        ));
    }
    let path = std::str::from_utf8(&payload[4..4 + path_len])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let data = &payload[4 + path_len..];
    Ok((path, data))
}
