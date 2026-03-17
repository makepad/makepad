use crate::VideoBitstreamFormat;

pub fn starts_with_annexb(data: &[u8]) -> bool {
    data.starts_with(&[0, 0, 1]) || data.starts_with(&[0, 0, 0, 1])
}

pub fn split_annexb_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0usize;

    fn find_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
        let mut i = from;
        while i + 3 <= data.len() {
            if i + 4 <= data.len()
                && data[i] == 0
                && data[i + 1] == 0
                && data[i + 2] == 0
                && data[i + 3] == 1
            {
                return Some((i, 4));
            }
            if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
                return Some((i, 3));
            }
            i += 1;
        }
        None
    }

    while let Some((sc, sc_len)) = find_start_code(data, i) {
        let nal_start = sc + sc_len;
        let next = find_start_code(data, nal_start)
            .map(|(p, _)| p)
            .unwrap_or(data.len());
        if nal_start < next {
            out.push(&data[nal_start..next]);
        }
        i = next;
    }

    out
}

pub fn contains_idr_annexb(data: &[u8]) -> bool {
    split_annexb_nals(data)
        .iter()
        .any(|nal| nal.first().map(|b| b & 0x1f == 5).unwrap_or(false))
}

pub fn annexb_to_sps_pps(data: &[u8]) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let mut sps = Vec::new();
    let mut pps = Vec::new();
    for nal in split_annexb_nals(data) {
        if nal.is_empty() {
            continue;
        }
        match nal[0] & 0x1f {
            7 => sps.push(nal.to_vec()),
            8 => pps.push(nal.to_vec()),
            _ => {}
        }
    }
    (sps, pps)
}

pub fn avcc_config_to_sps_pps(avcc: &[u8]) -> Option<(Vec<Vec<u8>>, Vec<Vec<u8>>, usize)> {
    if avcc.len() < 7 || avcc[0] != 1 {
        return None;
    }
    let nal_len_size = ((avcc[4] & 0x03) as usize) + 1;
    let num_sps = (avcc[5] & 0x1f) as usize;
    let mut o = 6usize;

    let mut sps = Vec::new();
    for _ in 0..num_sps {
        if o + 2 > avcc.len() {
            return None;
        }
        let len = u16::from_be_bytes([avcc[o], avcc[o + 1]]) as usize;
        o += 2;
        if o + len > avcc.len() {
            return None;
        }
        sps.push(avcc[o..o + len].to_vec());
        o += len;
    }

    if o >= avcc.len() {
        return None;
    }
    let num_pps = avcc[o] as usize;
    o += 1;

    let mut pps = Vec::new();
    for _ in 0..num_pps {
        if o + 2 > avcc.len() {
            return None;
        }
        let len = u16::from_be_bytes([avcc[o], avcc[o + 1]]) as usize;
        o += 2;
        if o + len > avcc.len() {
            return None;
        }
        pps.push(avcc[o..o + len].to_vec());
        o += len;
    }

    Some((sps, pps, nal_len_size))
}

pub fn sps_pps_to_annexb(sps: &[Vec<u8>], pps: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in sps.iter().chain(pps.iter()) {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
    out
}

pub fn avcc_sample_to_annexb(data: &[u8], nal_len_size: usize) -> Option<Vec<u8>> {
    if !(1..=4).contains(&nal_len_size) {
        return None;
    }
    let mut o = 0usize;
    let mut out = Vec::with_capacity(data.len() + 16);
    while o + nal_len_size <= data.len() {
        let mut len = 0usize;
        for b in &data[o..o + nal_len_size] {
            len = (len << 8) | (*b as usize);
        }
        o += nal_len_size;
        if o + len > data.len() {
            return None;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&data[o..o + len]);
        o += len;
    }
    if o == data.len() { Some(out) } else { None }
}

pub fn normalize_h264_config_to_annexb(
    format_hint: VideoBitstreamFormat,
    data: &[u8],
) -> Option<Vec<u8>> {
    match format_hint {
        VideoBitstreamFormat::AnnexB => Some(data.to_vec()),
        VideoBitstreamFormat::Avcc => {
            if let Some((sps, pps, _)) = avcc_config_to_sps_pps(data) {
                Some(sps_pps_to_annexb(&sps, &pps))
            } else {
                None
            }
        }
        _ => None,
    }
}
