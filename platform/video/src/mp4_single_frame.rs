//! An mp4 holding exactly one intra frame, written by hand.
//!
//! The platform's own file encoder is an `AVAssetWriter`, and for a single
//! still that machinery costs about fifty times what the encode does: on an
//! M3 Max, 13 ms to stand a writer up and 34 ms to finalize the container,
//! around 0.2 ms of actual HEVC. A picture cache that writes one file per
//! picture pays that on every picture.
//!
//! There is nothing to negotiate in the container for a single sample: every
//! table has one entry and every duration is one frame. So we write the boxes
//! ourselves and hand the encoded bits straight through. The layout is
//! `ftyp`/`mdat`/`moov` — `moov` last, so the sample's file offset is known
//! before the tables that point at it are built.

/// A box: 32-bit size, four-character name, body.
fn atom(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(body);
    out
}

/// A full box: an [`atom`] whose body opens with a version and 24-bit flags.
fn full_atom(name: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut inner = Vec::with_capacity(body.len() + 4);
    inner.push(version);
    inner.extend_from_slice(&flags.to_be_bytes()[1..]);
    inner.extend_from_slice(body);
    atom(name, &inner)
}

/// The unity display matrix, as 16.16 / 2.30 fixed point.
const IDENTITY_MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

fn matrix_bytes() -> Vec<u8> {
    IDENTITY_MATRIX.iter().flat_map(|v| v.to_be_bytes()).collect()
}

/// What the codec put out for one frame, and how to describe it.
pub struct IntraSample<'a> {
    /// The access unit exactly as VideoToolbox produced it: NAL units each
    /// preceded by a big-endian length of `length_size` bytes, which is what
    /// an mp4 sample is. No start-code conversion happens anywhere here.
    pub data: &'a [u8],
    /// The codec configuration atom — `hvcC` for HEVC, `avcC` for H.264 —
    /// taken verbatim from the encoder's format description, so the profile,
    /// level and parameter sets are the encoder's own account of itself
    /// rather than something we re-derived from the bitstream.
    pub config: &'a [u8],
    /// The four-character sample entry name: `hvc1` or `avc1`.
    pub sample_entry: [u8; 4],
    /// The four-character name of the configuration atom: `hvcC` or `avcC`.
    pub config_atom: [u8; 4],
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// The visual sample entry (`hvc1`/`avc1`) carrying the codec config atom.
fn sample_entry(sample: &IntraSample) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&[0u8; 6]); // reserved
    body.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    body.extend_from_slice(&[0u8; 2]); // pre_defined
    body.extend_from_slice(&[0u8; 2]); // reserved
    body.extend_from_slice(&[0u8; 12]); // pre_defined[3]
    body.extend_from_slice(&(sample.width as u16).to_be_bytes());
    body.extend_from_slice(&(sample.height as u16).to_be_bytes());
    body.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // 72 dpi horizontal
    body.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // 72 dpi vertical
    body.extend_from_slice(&[0u8; 4]); // reserved
    body.extend_from_slice(&1u16.to_be_bytes()); // frame_count
    body.extend_from_slice(&[0u8; 32]); // compressorname
    body.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
    body.extend_from_slice(&0xFFFFu16.to_be_bytes()); // pre_defined
    body.extend_from_slice(&atom(&sample.config_atom, sample.config));
    atom(&sample.sample_entry, &body)
}

/// The sample table. One sample, one chunk, one of everything.
fn stbl(sample: &IntraSample, sample_offset: u32, duration: u32) -> Vec<u8> {
    let stsd = {
        let mut body = 1u32.to_be_bytes().to_vec(); // entry_count
        body.extend_from_slice(&sample_entry(sample));
        full_atom(b"stsd", 0, 0, &body)
    };
    let stts = {
        let mut body = 1u32.to_be_bytes().to_vec(); // entry_count
        body.extend_from_slice(&1u32.to_be_bytes()); // sample_count
        body.extend_from_slice(&duration.to_be_bytes()); // sample_delta
        full_atom(b"stts", 0, 0, &body)
    };
    let stsc = {
        let mut body = 1u32.to_be_bytes().to_vec(); // entry_count
        body.extend_from_slice(&1u32.to_be_bytes()); // first_chunk
        body.extend_from_slice(&1u32.to_be_bytes()); // samples_per_chunk
        body.extend_from_slice(&1u32.to_be_bytes()); // sample_description_index
        full_atom(b"stsc", 0, 0, &body)
    };
    let stsz = {
        let mut body = 0u32.to_be_bytes().to_vec(); // sample_size: 0 = per-sample
        body.extend_from_slice(&1u32.to_be_bytes()); // sample_count
        body.extend_from_slice(&(sample.data.len() as u32).to_be_bytes());
        full_atom(b"stsz", 0, 0, &body)
    };
    let stco = {
        let mut body = 1u32.to_be_bytes().to_vec(); // entry_count
        body.extend_from_slice(&sample_offset.to_be_bytes());
        full_atom(b"stco", 0, 0, &body)
    };
    // The one sample is a sync sample; without this an intra-only track can
    // be taken for a track with no seek points at all.
    let stss = {
        let mut body = 1u32.to_be_bytes().to_vec(); // entry_count
        body.extend_from_slice(&1u32.to_be_bytes()); // sample number, 1-based
        full_atom(b"stss", 0, 0, &body)
    };
    let mut body = Vec::new();
    for part in [stsd, stts, stsc, stsz, stco, stss] {
        body.extend_from_slice(&part);
    }
    atom(b"stbl", &body)
}

/// Movie and media both count in this many units per second.
const TIMESCALE: u32 = 600;

/// Build the whole file: `ftyp`, then the sample in `mdat`, then `moov`.
pub fn write_single_frame_mp4(sample: &IntraSample) -> Vec<u8> {
    let ftyp = {
        let mut body = Vec::new();
        body.extend_from_slice(b"isom"); // major_brand
        body.extend_from_slice(&512u32.to_be_bytes()); // minor_version
        for brand in [b"isom", b"iso2", b"mp41"] {
            body.extend_from_slice(brand);
        }
        atom(b"ftyp", &body)
    };

    // `moov` comes last, so the sample's offset is simply where `mdat`'s
    // payload starts and nothing has to be patched afterwards.
    let sample_offset = (ftyp.len() + 8) as u32;
    let mut mdat = Vec::with_capacity(sample.data.len() + 8);
    mdat.extend_from_slice(&((sample.data.len() + 8) as u32).to_be_bytes());
    mdat.extend_from_slice(b"mdat");
    mdat.extend_from_slice(sample.data);

    let duration = (TIMESCALE / sample.fps.max(1)).max(1);

    let mvhd = {
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        body.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        body.extend_from_slice(&TIMESCALE.to_be_bytes());
        body.extend_from_slice(&duration.to_be_bytes());
        body.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
        body.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
        body.extend_from_slice(&[0u8; 2]); // reserved
        body.extend_from_slice(&[0u8; 8]); // reserved
        body.extend_from_slice(&matrix_bytes());
        body.extend_from_slice(&[0u8; 24]); // pre_defined
        body.extend_from_slice(&2u32.to_be_bytes()); // next_track_ID
        full_atom(b"mvhd", 0, 0, &body)
    };

    let tkhd = {
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        body.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        body.extend_from_slice(&1u32.to_be_bytes()); // track_ID
        body.extend_from_slice(&[0u8; 4]); // reserved
        body.extend_from_slice(&duration.to_be_bytes());
        body.extend_from_slice(&[0u8; 8]); // reserved
        body.extend_from_slice(&0u16.to_be_bytes()); // layer
        body.extend_from_slice(&0u16.to_be_bytes()); // alternate_group
        body.extend_from_slice(&0u16.to_be_bytes()); // volume: silent
        body.extend_from_slice(&[0u8; 2]); // reserved
        body.extend_from_slice(&matrix_bytes());
        body.extend_from_slice(&(sample.width << 16).to_be_bytes()); // 16.16
        body.extend_from_slice(&(sample.height << 16).to_be_bytes());
        // enabled | in movie | in preview
        full_atom(b"tkhd", 0, 0x7, &body)
    };

    let mdhd = {
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_be_bytes()); // creation_time
        body.extend_from_slice(&0u32.to_be_bytes()); // modification_time
        body.extend_from_slice(&TIMESCALE.to_be_bytes());
        body.extend_from_slice(&duration.to_be_bytes());
        body.extend_from_slice(&0x55C4u16.to_be_bytes()); // language: und
        body.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
        full_atom(b"mdhd", 0, 0, &body)
    };

    let hdlr = {
        let mut body = Vec::new();
        body.extend_from_slice(&[0u8; 4]); // pre_defined
        body.extend_from_slice(b"vide"); // handler_type
        body.extend_from_slice(&[0u8; 12]); // reserved
        body.extend_from_slice(b"VideoHandler\0");
        full_atom(b"hdlr", 0, 0, &body)
    };

    let minf = {
        let vmhd = {
            let mut body = 0u16.to_be_bytes().to_vec(); // graphicsmode
            body.extend_from_slice(&[0u8; 6]); // opcolor
            full_atom(b"vmhd", 0, 1, &body)
        };
        // A self-contained file: the one data reference is the file itself.
        let dinf = {
            let url = full_atom(b"url ", 0, 1, &[]);
            let mut body = 1u32.to_be_bytes().to_vec();
            body.extend_from_slice(&url);
            atom(b"dinf", &full_atom(b"dref", 0, 0, &body))
        };
        let mut body = Vec::new();
        body.extend_from_slice(&vmhd);
        body.extend_from_slice(&dinf);
        body.extend_from_slice(&stbl(sample, sample_offset, duration));
        atom(b"minf", &body)
    };

    let mdia = {
        let mut body = Vec::new();
        body.extend_from_slice(&mdhd);
        body.extend_from_slice(&hdlr);
        body.extend_from_slice(&minf);
        atom(b"mdia", &body)
    };

    let trak = {
        let mut body = Vec::new();
        body.extend_from_slice(&tkhd);
        body.extend_from_slice(&mdia);
        atom(b"trak", &body)
    };

    let moov = {
        let mut body = Vec::new();
        body.extend_from_slice(&mvhd);
        body.extend_from_slice(&trak);
        atom(b"moov", &body)
    };

    let mut out = Vec::with_capacity(ftyp.len() + mdat.len() + moov.len());
    out.extend_from_slice(&ftyp);
    out.extend_from_slice(&mdat);
    out.extend_from_slice(&moov);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bytes() -> Vec<u8> {
        // Two length-prefixed NAL units, as VideoToolbox hands them over.
        let mut data = Vec::new();
        data.extend_from_slice(&4u32.to_be_bytes());
        data.extend_from_slice(&[0x26, 0x01, 0xAF, 0x00]);
        data.extend_from_slice(&3u32.to_be_bytes());
        data.extend_from_slice(&[0x02, 0x01, 0xD0]);
        data
    }

    fn built() -> Vec<u8> {
        let data = sample_bytes();
        write_single_frame_mp4(&IntraSample {
            data: &data,
            config: &[0x01, 0x22, 0x00, 0x00],
            sample_entry: *b"hvc1",
            config_atom: *b"hvcC",
            width: 640,
            height: 480,
            fps: 30,
        })
    }

    /// Walk the top level and check the boxes are the ones we meant, each
    /// declaring its true length — a size that lies is the classic way a
    /// hand-built container fails, and it fails silently.
    fn top_level(file: &[u8]) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        let mut at = 0;
        while at + 8 <= file.len() {
            let size = u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize;
            let name = String::from_utf8_lossy(&file[at + 4..at + 8]).to_string();
            assert!(size >= 8 && at + size <= file.len(), "{name} claims {size} bytes");
            out.push((name, size));
            at += size;
        }
        assert_eq!(at, file.len(), "boxes do not tile the file exactly");
        out
    }

    #[test]
    fn boxes_tile_the_file() {
        let names: Vec<String> = top_level(&built()).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["ftyp", "mdat", "moov"]);
    }

    #[test]
    fn chunk_offset_points_at_the_sample() {
        let file = built();
        let data = sample_bytes();
        // stco holds one 32-bit offset; the bytes there must be the sample.
        let stco = file.windows(4).position(|w| w == b"stco").expect("stco present");
        // name, then version+flags, entry_count, offset
        let off_at = stco + 4 + 4 + 4;
        let offset = u32::from_be_bytes(file[off_at..off_at + 4].try_into().unwrap()) as usize;
        assert_eq!(&file[offset..offset + data.len()], &data[..], "stco does not point at the sample");
    }

    #[test]
    fn declared_sample_size_matches() {
        let file = built();
        let stsz = file.windows(4).position(|w| w == b"stsz").expect("stsz present");
        // name, version+flags, sample_size, sample_count, then the entry
        let size_at = stsz + 4 + 4 + 4 + 4;
        let size = u32::from_be_bytes(file[size_at..size_at + 4].try_into().unwrap()) as usize;
        assert_eq!(size, sample_bytes().len());
    }
}
