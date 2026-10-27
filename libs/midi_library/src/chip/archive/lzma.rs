//! LZMA and LZMA2 decoding (the 7z archive's coders), into memory: a
//! range decoder, the literal, match and repeat models, the length and
//! distance coders; LZMA2's chunks (stored or LZMA, with their state,
//! property and dictionary resets) over one decoder.

const PROB_INIT: u16 = 1024;
const TOP: u32 = 1 << 24;

struct Range<'a> {
    data: &'a [u8],
    pos: usize,
    range: u32,
    code: u32,
}

impl<'a> Range<'a> {
    fn new(data: &'a [u8]) -> Result<Range<'a>, String> {
        if data.len() < 5 {
            return Err("lzma: stream too short".into());
        }
        let code = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
        Ok(Range { data, pos: 5, range: 0xFFFF_FFFF, code })
    }

    #[inline]
    fn byte(&mut self) -> u8 {
        let b = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        b
    }

    #[inline]
    fn normalize(&mut self) {
        if self.range < TOP {
            self.range <<= 8;
            self.code = (self.code << 8) | self.byte() as u32;
        }
    }

    #[inline]
    fn bit(&mut self, prob: &mut u16) -> u32 {
        let bound = (self.range >> 11) * *prob as u32;
        let bit = if self.code < bound {
            self.range = bound;
            *prob += (2048 - *prob) >> 5;
            0
        } else {
            self.range -= bound;
            self.code -= bound;
            *prob -= *prob >> 5;
            1
        };
        self.normalize();
        bit
    }

    fn direct(&mut self, count: u32) -> u32 {
        let mut res = 0;
        for _ in 0..count {
            self.range >>= 1;
            let t = if self.code >= self.range { 1 } else { 0 };
            if t == 1 {
                self.code -= self.range;
            }
            res = (res << 1) | t;
            self.normalize();
        }
        res
    }

    fn tree(&mut self, probs: &mut [u16], bits: u32) -> u32 {
        let mut m = 1usize;
        for _ in 0..bits {
            m = (m << 1) | self.bit(&mut probs[m]) as usize;
        }
        (m - (1 << bits)) as u32
    }

    fn reverse(&mut self, probs: &mut [u16], bits: u32) -> u32 {
        let mut m = 1usize;
        let mut sym = 0;
        for i in 0..bits {
            let b = self.bit(&mut probs[m]);
            m = (m << 1) | b as usize;
            sym |= b << i;
        }
        sym
    }
}

struct LenDecoder {
    choice: u16,
    choice2: u16,
    low: Vec<[u16; 8]>,
    mid: Vec<[u16; 8]>,
    high: [u16; 256],
}

impl LenDecoder {
    fn new() -> LenDecoder {
        LenDecoder { choice: PROB_INIT, choice2: PROB_INIT, low: vec![[PROB_INIT; 8]; 16], mid: vec![[PROB_INIT; 8]; 16], high: [PROB_INIT; 256] }
    }

    fn decode(&mut self, rc: &mut Range, pos_state: usize) -> u32 {
        if rc.bit(&mut self.choice) == 0 {
            return rc.tree(&mut self.low[pos_state], 3);
        }
        if rc.bit(&mut self.choice2) == 0 {
            return 8 + rc.tree(&mut self.mid[pos_state], 3);
        }
        16 + rc.tree(&mut self.high, 8)
    }
}

/// The models of one LZMA stream (kept across LZMA2 chunks unless reset).
pub struct Lzma {
    lc: u32,
    lp: u32,
    pb: u32,
    literal: Vec<u16>,
    is_match: Vec<u16>,
    is_rep: [u16; 12],
    is_rep0: [u16; 12],
    is_rep1: [u16; 12],
    is_rep2: [u16; 12],
    is_rep0_long: Vec<u16>,
    dist_slot: Vec<[u16; 64]>,
    dist_special: [u16; 115],
    align: [u16; 16],
    len: LenDecoder,
    rep_len: LenDecoder,
    state: usize,
    reps: [u32; 4],
}

impl Lzma {
    pub fn new(props: u8) -> Result<Lzma, String> {
        if props >= 9 * 5 * 5 {
            return Err("lzma: bad properties".into());
        }
        let lc = (props % 9) as u32;
        let lp = ((props / 9) % 5) as u32;
        let pb = (props / 45) as u32;
        Ok(Lzma {
            lc,
            lp,
            pb,
            literal: vec![PROB_INIT; 0x300 << (lc + lp)],
            is_match: vec![PROB_INIT; 12 << 4],
            is_rep: [PROB_INIT; 12],
            is_rep0: [PROB_INIT; 12],
            is_rep1: [PROB_INIT; 12],
            is_rep2: [PROB_INIT; 12],
            is_rep0_long: vec![PROB_INIT; 12 << 4],
            dist_slot: vec![[PROB_INIT; 64]; 4],
            dist_special: [PROB_INIT; 115],
            align: [PROB_INIT; 16],
            len: LenDecoder::new(),
            rep_len: LenDecoder::new(),
            state: 0,
            reps: [0; 4],
        })
    }

    fn reset_state(&mut self, props: Option<u8>) -> Result<(), String> {
        let props = props.unwrap_or((self.pb * 5 + self.lp) as u8 * 9 + self.lc as u8);
        *self = Lzma::new(props)?;
        Ok(())
    }

    /// Decode `size` bytes (or to the end marker, for `None`) from `data`
    /// onto `out` (the dictionary is `out` itself, from `dict_start`).
    pub fn decode(&mut self, data: &[u8], out: &mut Vec<u8>, dict_start: usize, size: Option<usize>) -> Result<usize, String> {
        let mut rc = Range::new(data)?;
        let target = size.map(|s| out.len() + s);
        let pb_mask = (1usize << self.pb) - 1;
        let lp_mask = (1usize << self.lp) - 1;
        loop {
            if target.is_some_and(|t| out.len() >= t) {
                break;
            }
            let pos = out.len();
            let pos_state = pos & pb_mask;
            let state = self.state;
            if rc.bit(&mut self.is_match[(state << 4) + pos_state]) == 0 {
                // A literal.
                let prev = if pos > dict_start { out[pos - 1] as usize } else { 0 };
                let base = 0x300 * (((pos & lp_mask) << self.lc) + (prev >> (8 - self.lc)));
                let probs = &mut self.literal[base..base + 0x300];
                let mut sym = 1usize;
                if state >= 7 {
                    let back = self.reps[0] as usize + 1;
                    let mut matched = if pos >= back + dict_start { out[pos - back] as usize } else { 0 };
                    while sym < 0x100 {
                        let match_bit = (matched >> 7) & 1;
                        matched <<= 1;
                        let bit = rc.bit(&mut probs[((1 + match_bit) << 8) + sym]) as usize;
                        sym = (sym << 1) | bit;
                        if match_bit != bit {
                            break;
                        }
                    }
                }
                while sym < 0x100 {
                    sym = (sym << 1) | rc.bit(&mut probs[sym]) as usize;
                }
                out.push(sym as u8);
                self.state = if state < 4 { 0 } else if state < 10 { state - 3 } else { state - 6 };
                continue;
            }
            let len;
            if rc.bit(&mut self.is_rep[state]) != 0 {
                if out.len() == dict_start {
                    return Err("lzma: a repeat before any data".into());
                }
                if rc.bit(&mut self.is_rep0[state]) == 0 {
                    if rc.bit(&mut self.is_rep0_long[(state << 4) + pos_state]) == 0 {
                        // A short repeat: one byte.
                        self.state = if state < 7 { 9 } else { 11 };
                        let b = out[out.len() - self.reps[0] as usize - 1];
                        out.push(b);
                        continue;
                    }
                } else {
                    let dist;
                    if rc.bit(&mut self.is_rep1[state]) == 0 {
                        dist = self.reps[1];
                    } else {
                        if rc.bit(&mut self.is_rep2[state]) == 0 {
                            dist = self.reps[2];
                        } else {
                            dist = self.reps[3];
                            self.reps[3] = self.reps[2];
                        }
                        self.reps[2] = self.reps[1];
                    }
                    self.reps[1] = self.reps[0];
                    self.reps[0] = dist;
                }
                len = self.rep_len.decode(&mut rc, pos_state);
                self.state = if state < 7 { 8 } else { 11 };
            } else {
                self.reps[3] = self.reps[2];
                self.reps[2] = self.reps[1];
                self.reps[1] = self.reps[0];
                len = self.len.decode(&mut rc, pos_state);
                self.state = if state < 7 { 7 } else { 10 };
                let len_state = (len as usize).min(3);
                let slot = rc.tree(&mut self.dist_slot[len_state], 6);
                let dist = if slot < 4 {
                    slot
                } else {
                    let bits = (slot >> 1) - 1;
                    let mut d = (2 | (slot & 1)) << bits;
                    if slot < 14 {
                        let base = (d - slot) as usize;
                        d += rc.reverse(&mut self.dist_special[base..], bits);
                    } else {
                        d += rc.direct(bits - 4) << 4;
                        d += rc.reverse(&mut self.align, 4);
                    }
                    d
                };
                if dist == 0xFFFF_FFFF {
                    // The end marker.
                    break;
                }
                self.reps[0] = dist;
            }
            let len = len as usize + 2;
            let dist = self.reps[0] as usize + 1;
            if dist > out.len() - dict_start {
                return Err("lzma: a distance before the start".into());
            }
            let from = out.len() - dist;
            let len = target.map_or(len, |t| len.min(t - out.len()));
            if dist >= len {
                out.extend_from_within(from..from + len);
            } else {
                for k in 0..len {
                    let b = out[from + k];
                    out.push(b);
                }
            }
        }
        Ok(rc.pos)
    }
}

/// An LZMA stream as 7z stores it: `props` (5 bytes: lc/lp/pb and the
/// dictionary size) and the data, `size` bytes out.
pub fn lzma(props: &[u8], data: &[u8], size: usize) -> Result<Vec<u8>, String> {
    let mut d = Lzma::new(*props.first().ok_or("lzma: no properties")?)?;
    let mut out = Vec::with_capacity(size);
    d.decode(data, &mut out, 0, Some(size))?;
    Ok(out)
}

/// An LZMA2 stream, `size` bytes out.
pub fn lzma2(data: &[u8], size: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(size);
    let mut at = 0;
    let mut d: Option<Lzma> = None;
    let mut dict_start = 0;
    while at < data.len() {
        let ctrl = data[at];
        at += 1;
        if ctrl == 0 {
            break;
        }
        if ctrl == 1 || ctrl == 2 {
            if ctrl == 1 {
                dict_start = out.len();
            }
            let n = u16::from_be_bytes([data[at], data[at + 1]]) as usize + 1;
            at += 2;
            out.extend_from_slice(data.get(at..at + n).ok_or("lzma2: truncated")?);
            at += n;
            continue;
        }
        if ctrl < 0x80 {
            return Err("lzma2: bad chunk".into());
        }
        let unpacked = (((ctrl & 0x1F) as usize) << 16) + u16::from_be_bytes([data[at], data[at + 1]]) as usize + 1;
        let packed = u16::from_be_bytes([data[at + 2], data[at + 3]]) as usize + 1;
        at += 4;
        let mode = (ctrl >> 5) & 3;
        let props = if mode >= 2 {
            let p = data[at];
            at += 1;
            Some(p)
        } else {
            None
        };
        if mode == 3 {
            dict_start = out.len();
        }
        match (&mut d, props) {
            (None, Some(p)) => d = Some(Lzma::new(p)?),
            (None, None) => return Err("lzma2: no properties".into()),
            (Some(dec), _) if mode >= 1 => dec.reset_state(props)?,
            _ => {}
        }
        let dec = d.as_mut().unwrap();
        let chunk = data.get(at..at + packed).ok_or("lzma2: truncated chunk")?;
        dec.decode(chunk, &mut out, dict_start, Some(unpacked))?;
        at += packed;
    }
    if out.len() != size {
        return Err(format!("lzma2: {} bytes out of {size}", out.len()));
    }
    Ok(out)
}
