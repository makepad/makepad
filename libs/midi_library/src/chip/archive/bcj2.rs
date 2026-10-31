//! BCJ2: 7-Zip's x86 branch converter with its four streams (the main
//! bytes, the CALL and JUMP targets, and a range-coded stream saying which
//! E8/E9/Jcc bytes were converted). Undone: the absolute targets made
//! relative again.

pub fn decode(main: &[u8], call: &[u8], jump: &[u8], rc: &[u8], size: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(size);
    let mut probs = [1024u16; 258];
    let mut rc_at = 0;
    let byte = |at: &mut usize| -> u8 {
        let b = rc.get(*at).copied().unwrap_or(0);
        *at += 1;
        b
    };
    let mut code: u32 = 0;
    let mut range: u32 = 0xFFFF_FFFF;
    for _ in 0..5 {
        code = (code << 8) | byte(&mut rc_at) as u32;
    }
    let (mut mi, mut ci, mut ji) = (0usize, 0usize, 0usize);
    let mut prev: u8 = 0;
    let is_j = |p: u8, b: u8| (b & 0xFE) == 0xE8 || (p == 0x0F && (b & 0xF0) == 0x80);
    while out.len() < size {
        let Some(&b) = main.get(mi) else { break };
        mi += 1;
        out.push(b);
        if !is_j(prev, b) {
            prev = b;
            continue;
        }
        if out.len() >= size {
            break;
        }
        let p = if b == 0xE8 { &mut probs[prev as usize] } else if b == 0xE9 { &mut probs[256] } else { &mut probs[257] };
        let bound = (range >> 11) * *p as u32;
        let bit = if code < bound {
            range = bound;
            *p += (2048 - *p) >> 5;
            0
        } else {
            range -= bound;
            code -= bound;
            *p -= *p >> 5;
            1
        };
        if range < (1 << 24) {
            range <<= 8;
            code = (code << 8) | byte(&mut rc_at) as u32;
        }
        if bit == 0 {
            prev = b;
            continue;
        }
        let (src, at) = if b == 0xE8 { (call, &mut ci) } else { (jump, &mut ji) };
        let v = src.get(*at..*at + 4).ok_or("bcj2: a target stream ran out")?;
        *at += 4;
        let abs = u32::from_be_bytes([v[0], v[1], v[2], v[3]]);
        let dest = abs.wrapping_sub(out.len() as u32 + 4);
        out.extend_from_slice(&dest.to_le_bytes());
        prev = (dest >> 24) as u8;
    }
    out.truncate(size);
    if out.len() != size {
        return Err(format!("bcj2: {} bytes out of {size}", out.len()));
    }
    Ok(out)
}
