use crate::{h264_packets, EncodedVideoPacketOwned, VideoBitstreamFormat, VideoCodec};

#[derive(Clone)]
struct H264Config {
    sps: Vec<Vec<u8>>,
    pps: Vec<Vec<u8>>,
}

#[derive(Clone)]
struct Sample {
    bytes: Vec<u8>,
    is_key: bool,
    config_id: u32,
}

#[derive(Clone)]
struct Av1Sample {
    bytes: Vec<u8>,
    is_key: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveH264Sample<'a> {
    pub format: VideoBitstreamFormat,
    pub data: &'a [u8],
    pub dts: u64,
    pub pts: u64,
    pub is_key: bool,
}

#[derive(Debug, Clone)]
pub struct LiveH264Fmp4Muxer {
    width: u16,
    height: u16,
    timescale: u32,
    sequence_number: u32,
    config: Option<Vec<u8>>,
}

impl LiveH264Fmp4Muxer {
    pub fn new(width: u16, height: u16, timescale: u32) -> Option<Self> {
        if width == 0 || height == 0 || timescale == 0 {
            return None;
        }
        Some(Self {
            width,
            height,
            timescale,
            sequence_number: 1,
            config: None,
        })
    }

    pub fn set_avcc_config(&mut self, data: &[u8]) -> bool {
        if parse_avcc_config(data).is_none() {
            return false;
        }
        self.config = Some(data.to_vec());
        true
    }

    pub fn set_config(&mut self, format: VideoBitstreamFormat, data: &[u8]) -> bool {
        let avcc = match format {
            VideoBitstreamFormat::AnnexB => {
                let (sps, pps) = h264_packets::annexb_to_sps_pps(data);
                if sps.is_empty() || pps.is_empty() {
                    return false;
                }
                build_avcc(&H264Config { sps, pps })
            }
            VideoBitstreamFormat::Avcc => {
                if looks_like_avcc_config(data) {
                    data.to_vec()
                } else {
                    return false;
                }
            }
            _ => return false,
        };
        self.set_avcc_config(&avcc)
    }

    pub fn has_config(&self) -> bool {
        self.config.is_some()
    }

    pub fn init_segment(&self) -> Option<Vec<u8>> {
        let config = self.config.as_deref()?;
        Some(build_live_h264_init_segment(
            self.width,
            self.height,
            self.timescale,
            config,
        ))
    }

    pub fn push_sample(&mut self, sample: LiveH264Sample<'_>) -> Option<Vec<u8>> {
        let config = self.config.as_deref()?;
        let bytes = match sample.format {
            VideoBitstreamFormat::AnnexB => annexb_to_avcc_sample(sample.data),
            VideoBitstreamFormat::Avcc | VideoBitstreamFormat::RawAccessUnit => {
                if looks_like_avcc_config(sample.data) {
                    return None;
                }
                Some(sample.data.to_vec())
            }
            _ => None,
        }?;

        let duration = sample.pts.saturating_sub(sample.dts);
        let cts_offset = sample.pts as i128 - sample.dts as i128;
        let fragment = build_live_h264_fragment(
            self.sequence_number,
            self.timescale,
            config,
            &bytes,
            sample.dts,
            duration.max(1) as u32,
            cts_offset as i32,
            sample.is_key,
        );
        self.sequence_number = self.sequence_number.saturating_add(1);
        Some(fragment)
    }
}

pub fn build_h264_mp4(
    width: u16,
    height: u16,
    fps_num: u32,
    fps_den: u32,
    packets: &[EncodedVideoPacketOwned],
) -> Option<Vec<u8>> {
    if packets.is_empty() || width == 0 || height == 0 || fps_num == 0 || fps_den == 0 {
        return None;
    }

    let mut configs: std::collections::BTreeMap<u32, H264Config> = std::collections::BTreeMap::new();
    let mut samples = Vec::new();

    for pkt in packets {
        if pkt.data.is_empty() {
            continue;
        }

        if pkt.is_config {
            if let Some(cfg) = parse_config(pkt.format, &pkt.data) {
                configs.insert(pkt.config_id, cfg);
            }
            continue;
        }

        if pkt.is_eos {
            continue;
        }

        let sample = match pkt.format {
            VideoBitstreamFormat::AnnexB => annexb_to_avcc_sample(&pkt.data),
            VideoBitstreamFormat::Avcc | VideoBitstreamFormat::RawAccessUnit => {
                if looks_like_avcc_config(&pkt.data) {
                    None
                } else {
                    Some(pkt.data.clone())
                }
            }
            _ => None,
        };

        if let Some(bytes) = sample {
            if !bytes.is_empty() {
                samples.push(Sample {
                    bytes,
                    is_key: pkt.is_key,
                    config_id: pkt.config_id,
                });
            }
        }
    }

    if samples.is_empty() {
        return None;
    }

    let config = samples
        .iter()
        .find_map(|s| configs.get(&s.config_id).cloned())
        .or_else(|| configs.values().next().cloned())?;

    if config.sps.is_empty() || config.pps.is_empty() {
        return None;
    }

    Some(mux_mp4(width, height, fps_num, fps_den, &config, &samples))
}

pub fn build_av1_mp4(
    width: u16,
    height: u16,
    fps_num: u32,
    fps_den: u32,
    packets: &[EncodedVideoPacketOwned],
) -> Option<Vec<u8>> {
    if packets.is_empty() || width == 0 || height == 0 || fps_num == 0 || fps_den == 0 {
        return None;
    }

    let mut samples = Vec::new();
    let mut seq_header_obu = None;

    for pkt in packets {
        if pkt.codec != VideoCodec::Av1 || pkt.data.is_empty() || pkt.is_eos || pkt.is_config {
            continue;
        }

        match pkt.format {
            VideoBitstreamFormat::Av1Obu | VideoBitstreamFormat::RawAccessUnit => {
                if seq_header_obu.is_none() {
                    seq_header_obu = extract_first_sequence_header_obu(&pkt.data);
                }
                samples.push(Av1Sample {
                    bytes: pkt.data.clone(),
                    is_key: pkt.is_key,
                });
            }
            _ => {}
        }
    }

    if samples.is_empty() {
        return None;
    }

    Some(mux_av1_mp4(
        width,
        height,
        fps_num,
        fps_den,
        &samples,
        seq_header_obu.as_deref(),
    ))
}

fn looks_like_avcc_config(data: &[u8]) -> bool {
    data.len() >= 7 && data[0] == 1
}

fn parse_config(format: VideoBitstreamFormat, data: &[u8]) -> Option<H264Config> {
    match format {
        VideoBitstreamFormat::AnnexB => {
            let (sps, pps) = h264_packets::annexb_to_sps_pps(data);
            if sps.is_empty() || pps.is_empty() {
                None
            } else {
                Some(H264Config { sps, pps })
            }
        }
        VideoBitstreamFormat::Avcc => parse_avcc_config(data),
        _ => None,
    }
}

fn parse_avcc_config(data: &[u8]) -> Option<H264Config> {
    let (sps, pps, _) = h264_packets::avcc_config_to_sps_pps(data)?;
    if sps.is_empty() || pps.is_empty() {
        None
    } else {
        Some(H264Config { sps, pps })
    }
}

fn annexb_to_avcc_sample(data: &[u8]) -> Option<Vec<u8>> {
    let nals = h264_packets::split_annexb_nals(data);
    if nals.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(data.len() + nals.len() * 4);
    for nal in nals {
        if nal.is_empty() {
            continue;
        }
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    if out.is_empty() { None } else { Some(out) }
}

fn boxed(name: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(payload);
    out
}

fn full_box(version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.push(version);
    out.extend_from_slice(&flags.to_be_bytes()[1..]);
    out.extend_from_slice(payload);
    out
}

fn write_visual_sample_entry(sample_entry: &[u8; 4], width: u16, height: u16, config_box: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0; 6]);
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&[0u8; 12]);
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    out.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&[0u8; 32]);
    out.extend_from_slice(&0x0018u16.to_be_bytes());
    out.extend_from_slice(&0xFFFFu16.to_be_bytes());
    out.extend_from_slice(&config_box);
    boxed(sample_entry, &out)
}

fn stsd_single(entry: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&full_box(0, 0, &1u32.to_be_bytes()));
    out.extend_from_slice(&entry);
    boxed(b"stsd", &out)
}

fn video_stbl(sample_table_entry: Vec<u8>, stts: Vec<u8>, stsc: Vec<u8>, stsz: Vec<u8>, stco: Vec<u8>, stss: Option<Vec<u8>>) -> Vec<u8> {
    let mut children = vec![
        stsd_single(sample_table_entry),
        boxed(b"stts", &stts),
        boxed(b"stsc", &stsc),
        boxed(b"stsz", &stsz),
        boxed(b"stco", &stco),
    ];
    if let Some(stss) = stss {
        children.push(boxed(b"stss", &stss));
    }
    boxed(b"stbl", &children.concat())
}

fn video_mdia(timescale: u32, duration: u32, minf: Vec<u8>) -> Vec<u8> {
    let mut mdhd = Vec::new();
    mdhd.extend_from_slice(&full_box(0, 0, &[]));
    mdhd.extend_from_slice(&0u32.to_be_bytes());
    mdhd.extend_from_slice(&0u32.to_be_bytes());
    mdhd.extend_from_slice(&timescale.to_be_bytes());
    mdhd.extend_from_slice(&duration.to_be_bytes());
    mdhd.extend_from_slice(&0x55c4u16.to_be_bytes());
    mdhd.extend_from_slice(&0u16.to_be_bytes());

    let hdlr = boxed(
        b"hdlr",
        &[
            0, 0, 0, 0,
            0, 0, 0, 0,
            b'v', b'i', b'd', b'e',
            0, 0, 0, 0,
            0, 0, 0, 0,
            0, 0, 0, 0,
            b'V', b'i', b'd', b'e', b'o', b'H', b'a', b'n', b'd', b'l', b'e', b'r', 0,
        ],
    );

    boxed(b"mdia", &[boxed(b"mdhd", &mdhd), hdlr, minf].concat())
}

fn video_minf(stbl: Vec<u8>) -> Vec<u8> {
    let vmhd = boxed(
        b"vmhd",
        &[
            0x00, 0x00, 0x00, 0x01,
            0, 0,
            0, 0, 0, 0, 0, 0,
        ],
    );

    let dref = boxed(
        b"dref",
        &[
            0, 0, 0, 0,
            0, 0, 0, 1,
            0, 0, 0, 12, b'u', b'r', b'l', b' ', 0, 0, 0, 1,
        ],
    );
    let dinf = boxed(b"dinf", &dref);

    boxed(b"minf", &[vmhd, dinf, stbl].concat())
}

const IDENTITY_MATRIX: [u8; 36] = [
    0x00, 0x01, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
    0x00, 0x01, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
    0x40, 0x00, 0x00, 0x00,
];

fn tkhd(track_id: u32, duration: u32, width: u16, height: u16) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0x00000007u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&track_id.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&duration.to_be_bytes());
    out.extend_from_slice(&[0u8; 8]);
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&IDENTITY_MATRIX);
    out.extend_from_slice(&((width as u32) << 16).to_be_bytes());
    out.extend_from_slice(&((height as u32) << 16).to_be_bytes());
    boxed(b"tkhd", &out)
}

fn mvhd(timescale: u32, duration: u32, next_track_id: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&timescale.to_be_bytes());
    out.extend_from_slice(&duration.to_be_bytes());
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&0x0100u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&[0u8; 8]);
    out.extend_from_slice(&IDENTITY_MATRIX);
    out.extend_from_slice(&[0u8; 24]);
    out.extend_from_slice(&next_track_id.to_be_bytes());
    boxed(b"mvhd", &out)
}

fn ftyp(brand: &[u8; 4]) -> Vec<u8> {
    boxed(
        b"ftyp",
        &[
            brand[0], brand[1], brand[2], brand[3],
            0, 0, 0, 0,
            b'i', b's', b'o', b'm',
            brand[0], brand[1], brand[2], brand[3],
            b'm', b'p', b'4', b'1',
        ],
    )
}

fn mux_mp4(
    width: u16,
    height: u16,
    fps_num: u32,
    fps_den: u32,
    config: &H264Config,
    samples: &[Sample],
) -> Vec<u8> {
    let timescale: u32 = 1000;
    let sample_delta = ((timescale as u64 * fps_den as u64) / fps_num as u64).max(1) as u32;
    let duration = sample_delta.saturating_mul(samples.len() as u32);

    let mut mdat_payload = Vec::new();
    let mut sample_sizes = Vec::with_capacity(samples.len());
    let mut sync_samples = Vec::new();
    for (idx, s) in samples.iter().enumerate() {
        sample_sizes.push(s.bytes.len() as u32);
        if s.is_key {
            sync_samples.push((idx + 1) as u32);
        }
        mdat_payload.extend_from_slice(&s.bytes);
    }

    if sync_samples.is_empty() {
        for i in 0..samples.len() {
            sync_samples.push((i + 1) as u32);
        }
    }

    let ftyp = ftyp(b"avc1");
    let mdat = boxed(b"mdat", &mdat_payload);
    let chunk_offset = (ftyp.len() + 8) as u32;

    let sample_entry = write_visual_sample_entry(b"avc1", width, height, boxed(b"avcC", &build_avcc(config)));

    let stts = [0u32.to_be_bytes(), 1u32.to_be_bytes(), (samples.len() as u32).to_be_bytes(), sample_delta.to_be_bytes()].concat();
    let stsc = [0u32.to_be_bytes(), 1u32.to_be_bytes(), 1u32.to_be_bytes(), (samples.len() as u32).to_be_bytes(), 1u32.to_be_bytes()].concat();
    let mut stsz = [0u32.to_be_bytes(), 0u32.to_be_bytes(), (samples.len() as u32).to_be_bytes()].concat();
    for sz in &sample_sizes {
        stsz.extend_from_slice(&sz.to_be_bytes());
    }
    let stco = [0u32.to_be_bytes(), 1u32.to_be_bytes(), chunk_offset.to_be_bytes()].concat();
    let mut stss = [0u32.to_be_bytes(), (sync_samples.len() as u32).to_be_bytes()].concat();
    for id in &sync_samples {
        stss.extend_from_slice(&id.to_be_bytes());
    }

    let stbl = video_stbl(sample_entry, stts, stsc, stsz, stco, Some(stss));
    let minf = video_minf(stbl);
    let mdia = video_mdia(timescale, duration, minf);
    let trak = boxed(b"trak", &[tkhd(1, duration, width, height), mdia].concat());
    let moov = boxed(b"moov", &[mvhd(timescale, duration, 2), trak].concat());

    [ftyp, mdat, moov].concat()
}

fn build_live_h264_init_segment(width: u16, height: u16, timescale: u32, avcc: &[u8]) -> Vec<u8> {
    let sample_entry = write_visual_sample_entry(b"avc1", width, height, boxed(b"avcC", avcc));
    let stbl = video_stbl(
        sample_entry,
        [0u32.to_be_bytes(), 0u32.to_be_bytes()].concat(),
        [0u32.to_be_bytes(), 0u32.to_be_bytes()].concat(),
        [0u32.to_be_bytes(), 0u32.to_be_bytes(), 0u32.to_be_bytes()].concat(),
        [0u32.to_be_bytes(), 0u32.to_be_bytes()].concat(),
        None,
    );
    let minf = video_minf(stbl);
    let mdia = video_mdia(timescale, 0, minf);
    let trak = boxed(b"trak", &[tkhd(1, 0, width, height), mdia].concat());

    let mut trex = Vec::new();
    trex.extend_from_slice(&0u32.to_be_bytes());
    trex.extend_from_slice(&1u32.to_be_bytes());
    trex.extend_from_slice(&1u32.to_be_bytes());
    trex.extend_from_slice(&1u32.to_be_bytes());
    trex.extend_from_slice(&0u32.to_be_bytes());
    trex.extend_from_slice(&0u32.to_be_bytes());
    let mvex = boxed(b"mvex", &boxed(b"trex", &trex));

    let moov = boxed(b"moov", &[mvhd(timescale, 0, 2), trak, mvex].concat());
    [ftyp(b"avc1"), moov].concat()
}

fn build_live_h264_fragment(
    sequence_number: u32,
    _timescale: u32,
    _avcc: &[u8],
    sample: &[u8],
    dts: u64,
    duration: u32,
    cts_offset: i32,
    is_key: bool,
) -> Vec<u8> {
    let sample_flags = if is_key { 0x00000000u32 } else { 0x00010000u32 };

    let mfhd = boxed(b"mfhd", &full_box(0, 0, &sequence_number.to_be_bytes()));

    let mut tfhd = Vec::new();
    tfhd.extend_from_slice(&1u32.to_be_bytes());
    let tfhd = boxed(b"tfhd", &full_box(0, 0x020000, &tfhd));

    let tfdt = boxed(b"tfdt", &full_box(1, 0, &dts.to_be_bytes()));

    let data_offset = 8 + mfhd.len() + (8 + tfhd.len() + tfdt.len() + (8 + 4 + 4 + 4 + 4 + 4 + 4));
    let mut trun = Vec::new();
    trun.extend_from_slice(&1u32.to_be_bytes());
    trun.extend_from_slice(&(data_offset as i32).to_be_bytes());
    trun.extend_from_slice(&duration.to_be_bytes());
    trun.extend_from_slice(&(sample.len() as u32).to_be_bytes());
    trun.extend_from_slice(&sample_flags.to_be_bytes());
    trun.extend_from_slice(&cts_offset.to_be_bytes());
    let trun = boxed(b"trun", &full_box(1, 0x000001 | 0x000100 | 0x000200 | 0x000400 | 0x000800, &trun));

    let traf = boxed(b"traf", &[tfhd, tfdt, trun].concat());
    let moof = boxed(b"moof", &[mfhd, traf].concat());
    let mdat = boxed(b"mdat", sample);
    [moof, mdat].concat()
}

fn build_avcc(config: &H264Config) -> Vec<u8> {
    let sps = &config.sps[0];
    let mut out = Vec::new();
    out.push(1);
    out.push(*sps.get(1).unwrap_or(&0x64));
    out.push(*sps.get(2).unwrap_or(&0));
    out.push(*sps.get(3).unwrap_or(&0x1f));
    out.push(0xFF);

    let sps_count = config.sps.len().min(31) as u8;
    out.push(0xE0 | sps_count);
    for s in &config.sps {
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s);
    }

    let pps_count = config.pps.len().min(255) as u8;
    out.push(pps_count);
    for p in &config.pps {
        out.extend_from_slice(&(p.len() as u16).to_be_bytes());
        out.extend_from_slice(p);
    }
    out
}

fn parse_leb128_usize(data: &[u8], offset: &mut usize) -> Option<usize> {
    let mut value = 0usize;
    let mut shift = 0usize;
    while *offset < data.len() && shift < (std::mem::size_of::<usize>() * 8) {
        let b = data[*offset];
        *offset += 1;
        value |= ((b & 0x7f) as usize) << shift;
        if (b & 0x80) == 0 {
            return Some(value);
        }
        shift += 7;
    }
    None
}

fn extract_first_sequence_header_obu(data: &[u8]) -> Option<Vec<u8>> {
    let mut i = 0usize;
    while i < data.len() {
        let start = i;
        let header = *data.get(i)?;
        i += 1;

        let obu_type = (header >> 3) & 0x0f;
        let has_extension = (header & 0x04) != 0;
        let has_size_field = (header & 0x02) != 0;

        if has_extension {
            i = i.saturating_add(1);
            if i > data.len() {
                return None;
            }
        }

        let payload_len = if has_size_field {
            parse_leb128_usize(data, &mut i)?
        } else {
            data.len().saturating_sub(i)
        };

        let end = i.saturating_add(payload_len);
        if end > data.len() {
            return None;
        }

        if obu_type == 1 {
            return Some(data[start..end].to_vec());
        }

        i = end;
    }

    None
}

fn build_av1c(sequence_header_obu: Option<&[u8]>) -> Vec<u8> {
    let mut out = vec![
        0x81,
        0x00,
        0x0c,
        0x00,
    ];
    if let Some(obu) = sequence_header_obu {
        out.extend_from_slice(obu);
    }
    out
}

fn mux_av1_mp4(
    width: u16,
    height: u16,
    fps_num: u32,
    fps_den: u32,
    samples: &[Av1Sample],
    sequence_header_obu: Option<&[u8]>,
) -> Vec<u8> {
    let timescale: u32 = 1000;
    let sample_delta = ((timescale as u64 * fps_den as u64) / fps_num as u64).max(1) as u32;
    let duration = sample_delta.saturating_mul(samples.len() as u32);

    let mut mdat_payload = Vec::new();
    let mut sample_sizes = Vec::with_capacity(samples.len());
    let mut sync_samples = Vec::new();
    for (idx, s) in samples.iter().enumerate() {
        sample_sizes.push(s.bytes.len() as u32);
        if s.is_key {
            sync_samples.push((idx + 1) as u32);
        }
        mdat_payload.extend_from_slice(&s.bytes);
    }

    if sync_samples.is_empty() {
        for i in 0..samples.len() {
            sync_samples.push((i + 1) as u32);
        }
    }

    let ftyp = ftyp(b"av01");
    let mdat = boxed(b"mdat", &mdat_payload);
    let chunk_offset = (ftyp.len() + 8) as u32;
    let sample_entry = write_visual_sample_entry(b"av01", width, height, boxed(b"av1C", &build_av1c(sequence_header_obu)));

    let stts = [0u32.to_be_bytes(), 1u32.to_be_bytes(), (samples.len() as u32).to_be_bytes(), sample_delta.to_be_bytes()].concat();
    let stsc = [0u32.to_be_bytes(), 1u32.to_be_bytes(), 1u32.to_be_bytes(), (samples.len() as u32).to_be_bytes(), 1u32.to_be_bytes()].concat();
    let mut stsz = [0u32.to_be_bytes(), 0u32.to_be_bytes(), (samples.len() as u32).to_be_bytes()].concat();
    for sz in &sample_sizes {
        stsz.extend_from_slice(&sz.to_be_bytes());
    }
    let stco = [0u32.to_be_bytes(), 1u32.to_be_bytes(), chunk_offset.to_be_bytes()].concat();
    let mut stss = [0u32.to_be_bytes(), (sync_samples.len() as u32).to_be_bytes()].concat();
    for id in &sync_samples {
        stss.extend_from_slice(&id.to_be_bytes());
    }

    let stbl = video_stbl(sample_entry, stts, stsc, stsz, stco, Some(stss));
    let minf = video_minf(stbl);
    let mdia = video_mdia(timescale, duration, minf);
    let trak = boxed(b"trak", &[tkhd(1, duration, width, height), mdia].concat());
    let moov = boxed(b"moov", &[mvhd(timescale, duration, 2), trak].concat());

    [ftyp, mdat, moov].concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fmp4_demux::{FMp4Codec, FMp4Event, IncrementalDemuxer};

    const SPS: &[u8] = &[0x67, 0x64, 0x00, 0x1f, 0xac, 0xd9, 0x40, 0x50, 0x1e, 0xd0, 0x0f, 0x12];
    const PPS: &[u8] = &[0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0];
    const IDR: &[u8] = &[0x65, 0x88, 0x84, 0x21];
    const PFRAME: &[u8] = &[0x41, 0x9a, 0x22, 0x11];

    fn annexb_config() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(SPS);
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(PPS);
        out
    }

    fn annexb_sample(nal: &[u8]) -> Vec<u8> {
        let mut out = vec![0, 0, 0, 1];
        out.extend_from_slice(nal);
        out
    }

    #[test]
    fn live_h264_init_segment_parses() {
        let mut muxer = LiveH264Fmp4Muxer::new(640, 360, 90000).unwrap();
        assert!(muxer.set_config(VideoBitstreamFormat::AnnexB, &annexb_config()));
        let init = muxer.init_segment().unwrap();

        let mut demuxer = IncrementalDemuxer::new();
        let events = demuxer.push_data(&init);
        assert_eq!(events.len(), 1);
        match &events[0] {
            FMp4Event::InitSegment(init) => {
                assert_eq!(init.video_tracks.len(), 1);
                let track = &init.video_tracks[0];
                assert_eq!(track.codec, FMp4Codec::H264);
                assert_eq!(track.width, 640);
                assert_eq!(track.height, 360);
                assert_eq!(track.timescale, 90000);
                assert_eq!(track.nal_length_size, 4);
                assert!(!track.codec_config.is_empty());
            }
            _ => panic!("expected init segment"),
        }
    }

    #[test]
    fn live_h264_fragments_parse_incrementally() {
        let mut muxer = LiveH264Fmp4Muxer::new(640, 360, 90000).unwrap();
        assert!(muxer.set_config(VideoBitstreamFormat::AnnexB, &annexb_config()));
        let init = muxer.init_segment().unwrap();
        let frag1 = muxer
            .push_sample(LiveH264Sample {
                format: VideoBitstreamFormat::AnnexB,
                data: &annexb_sample(IDR),
                dts: 0,
                pts: 3000,
                is_key: true,
            })
            .unwrap();
        let frag2 = muxer
            .push_sample(LiveH264Sample {
                format: VideoBitstreamFormat::AnnexB,
                data: &annexb_sample(PFRAME),
                dts: 3000,
                pts: 6000,
                is_key: false,
            })
            .unwrap();

        let mut demuxer = IncrementalDemuxer::new();
        assert!(matches!(demuxer.push_data(&init)[0], FMp4Event::InitSegment(_)));

        let events = demuxer.push_data(&frag1);
        match &events[0] {
            FMp4Event::MediaSamples(samples) => {
                assert_eq!(samples.len(), 1);
                assert_eq!(samples[0].dts, 0);
                assert_eq!(samples[0].pts, 3000);
                assert_eq!(samples[0].duration, 3000);
                assert!(samples[0].is_sync);
                assert_eq!(samples[0].data, annexb_to_avcc_sample(&annexb_sample(IDR)).unwrap());
            }
            _ => panic!("expected samples"),
        }

        let events = demuxer.push_data(&frag2);
        match &events[0] {
            FMp4Event::MediaSamples(samples) => {
                assert_eq!(samples.len(), 1);
                assert_eq!(samples[0].dts, 3000);
                assert_eq!(samples[0].pts, 6000);
                assert_eq!(samples[0].duration, 3000);
                assert!(!samples[0].is_sync);
            }
            _ => panic!("expected samples"),
        }
    }

    #[test]
    fn live_h264_sequence_and_config_handling_is_sane() {
        let mut muxer = LiveH264Fmp4Muxer::new(1280, 720, 1000).unwrap();
        assert!(muxer.init_segment().is_none());
        assert!(!muxer.set_config(VideoBitstreamFormat::Av1Obu, &[1, 2, 3]));
        assert!(muxer.set_avcc_config(&build_avcc(&H264Config {
            sps: vec![SPS.to_vec()],
            pps: vec![PPS.to_vec()],
        })));

        let frag1 = muxer
            .push_sample(LiveH264Sample {
                format: VideoBitstreamFormat::Avcc,
                data: &annexb_to_avcc_sample(&annexb_sample(IDR)).unwrap(),
                dts: 10,
                pts: 15,
                is_key: true,
            })
            .unwrap();
        let frag2 = muxer
            .push_sample(LiveH264Sample {
                format: VideoBitstreamFormat::Avcc,
                data: &annexb_to_avcc_sample(&annexb_sample(PFRAME)).unwrap(),
                dts: 20,
                pts: 25,
                is_key: false,
            })
            .unwrap();

        fn find_box<'a>(data: &'a [u8], name: &[u8; 4]) -> Option<&'a [u8]> {
            let mut o = 0usize;
            while o + 8 <= data.len() {
                let size = u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as usize;
                if size < 8 || o + size > data.len() {
                    return None;
                }
                if &data[o + 4..o + 8] == name {
                    return Some(&data[o + 8..o + size]);
                }
                o += size;
            }
            None
        }

        let mfhd1 = find_box(&frag1, b"moof").and_then(|moof| find_box(moof, b"mfhd")).unwrap();
        let mfhd2 = find_box(&frag2, b"moof").and_then(|moof| find_box(moof, b"mfhd")).unwrap();
        assert_eq!(u32::from_be_bytes([mfhd1[4], mfhd1[5], mfhd1[6], mfhd1[7]]), 1);
        assert_eq!(u32::from_be_bytes([mfhd2[4], mfhd2[5], mfhd2[6], mfhd2[7]]), 2);
    }
}
