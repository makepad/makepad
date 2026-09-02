/// Read one protobuf varint and advance `pos` past it.
pub fn read_pb_varint(bytes: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    while *pos < bytes.len() {
        let byte = bytes[*pos];
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift > 63 {
            return Err("varint too long".to_string());
        }
    }
    Err("unexpected eof reading varint".to_string())
}

/// Read one protobuf length-delimited payload and advance `pos` past it.
pub fn read_pb_len_slice<'a>(bytes: &'a [u8], pos: &mut usize) -> Result<&'a [u8], String> {
    let len = usize::try_from(read_pb_varint(bytes, pos)?)
        .map_err(|_| "protobuf length does not fit usize".to_string())?;
    let end = pos
        .checked_add(len)
        .ok_or_else(|| "protobuf length overflow".to_string())?;
    let slice = bytes
        .get(*pos..end)
        .ok_or_else(|| "unexpected eof reading length-delimited field".to_string())?;
    *pos = end;
    Ok(slice)
}

/// Skip one protobuf field payload after its key has already been read.
pub fn skip_pb_field(bytes: &[u8], pos: &mut usize, wire: u8) -> Result<(), String> {
    let fixed = match wire {
        0 => {
            let _ = read_pb_varint(bytes, pos)?;
            return Ok(());
        }
        1 => 8,
        2 => {
            let _ = read_pb_len_slice(bytes, pos)?;
            return Ok(());
        }
        5 => 4,
        _ => return Err(format!("unsupported protobuf wire type {wire}")),
    };
    *pos = pos
        .checked_add(fixed)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| "unexpected eof skipping fixed-width field".to_string())?;
    Ok(())
}

