//! "Save clip": a range of a [`Snapshot`] into the checkout's screencap folder
//! (`local/screencap`), on a thread of its own:
//! - per stream (one per tile band, or per interleaved engine) the received bitstream as it
//!   came, no re-encode: an MP4 (`-s<N>.mp4`) and the raw Annex-B (`-s<N>.h264`, or `.h265`; all of them,
//!   comma separated, play in the viewer with `--dump-play`). Both start at the key run before
//!   the range, so they decode on their own.
//! - PNGs of the pictures in the range (at most [`MAX_PNGS`], centred on the shown picture),
//!   full pictures with the tiles put together (`<name>-png/f<frame>.png`).
//! - Notes for whoever reads them (`<name>.md`): what is unusual first (gaps, skipped frame
//!   numbers, size spikes, key frames, failed decodes, each with its frame number), then the
//!   files; every picture's numbers in `<name>.tsv`.

use crate::history::Snapshot;
use makepad_network::SignalToUI;
use makepad_widgets::makepad_platform::video_file::{annex_b, hevc_config, VideoStreamDecoder};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The most PNGs one save writes (each a full picture: about 1-3 MB at 4K).
pub const MAX_PNGS: usize = 240;
/// The PNGs of a save without a chosen range: this many pictures each side of the shown one.
pub const PNGS_AROUND: usize = 30;

pub struct ClipRequest {
    pub snap: Arc<Snapshot>,
    /// The pictures (indices into the snapshot, inclusive).
    pub from: usize,
    pub to: usize,
    /// The picture shown when saving (the PNGs centre on it).
    pub shown: usize,
    /// Only the shown picture's PNG and its notes (no bitstream).
    pub frame_only: bool,
    /// The range was chosen (In / Out): PNGs of all of it (up to [`MAX_PNGS`]); else
    /// [`PNGS_AROUND`] each side of the shown picture.
    pub chosen: bool,
    pub host: String,
}

/// Starts the save; `status` says how far it is and, at the end, where it went.
pub fn save(req: ClipRequest, status: Arc<Mutex<String>>, signal: SignalToUI) {
    let set = {
        let (status, signal) = (status.clone(), signal.clone());
        move |s: String| {
            *status.lock().unwrap() = s;
            signal.set();
        }
    };
    set("saving…".into());
    let _ = std::thread::Builder::new().name("desklink-clip".into()).spawn(move || {
        let dir = makepad_widgets::repo_screencap_dir();
        match write_clip(&req, &dir, &set) {
            Ok(path) => {
                eprintln!("desklink: clip saved: {}", path.display());
                set(format!("saved local/screencap/{}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
            }
            Err(e) => set(format!("save failed: {e}")),
        }
    });
}

fn write_clip(req: &ClipRequest, dir: &Path, set: &dyn Fn(String)) -> Result<PathBuf, String> {
    let snap = &req.snap;
    if snap.len() == 0 {
        return Err("the history is empty".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let (from, to) = if req.frame_only { (req.shown, req.shown) } else { (req.from.min(req.to), req.from.max(req.to).min(snap.len() - 1)) };
    let host: String = req.host.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    let name = format!("desklink-{host}-{}-f{}-{}", utc_stamp(), snap.pictures[from].frame, snap.pictures[to].frame);
    let base = dir.join(&name);
    let sync = snap.pictures[from].sync.ok_or_else(|| format!("frame {} has no key frame before it in the history", snap.pictures[from].frame))?;

    // the PNGs: the range, at most MAX_PNGS of it around the shown picture
    let (p0, p1) = if !req.chosen {
        let at = req.shown.clamp(from, to);
        (at.saturating_sub(PNGS_AROUND).max(from), (at + PNGS_AROUND).min(to))
    } else if to - from < MAX_PNGS {
        (from, to)
    } else {
        let half = MAX_PNGS / 2;
        let p0 = req.shown.clamp(from, to).saturating_sub(half).max(from);
        (p0, (p0 + MAX_PNGS - 1).min(to))
    };
    let png_dir = PathBuf::from(format!("{}-png", base.display()));
    std::fs::create_dir_all(&png_dir).map_err(|e| format!("{}: {e}", png_dir.display()))?;
    let pngs = write_pngs(snap, sync, p0, p1, &png_dir, set)?;

    let mut files = Vec::new();
    if !req.frame_only {
        let streams = snap.packets[snap.pictures[sync].first..snap.pictures[to].first + snap.pictures[to].count].iter().map(|p| p.stream as usize + 1).max().unwrap_or(1);
        for s in 0..streams {
            let mut packets: Vec<_> = (sync..=to).flat_map(|i| snap.packets_of(i).iter()).filter(|p| p.stream as usize == s).collect();
            // one codec per file: the key run's (a desktop that switched codec in the range: up
            // to the switch)
            let hevc = packets.first().is_some_and(|p| p.hevc());
            if let Some(end) = packets.iter().position(|p| p.hevc() != hevc) {
                packets.truncate(end);
            }
            let mut raw = Vec::new();
            for p in &packets {
                raw.extend_from_slice(&p.data);
            }
            let annex = PathBuf::from(format!("{}-s{s}.{}", base.display(), if hevc { "h265" } else { "h264" }));
            std::fs::write(&annex, &raw).map_err(|e| format!("{}: {e}", annex.display()))?;
            files.push(annex);
            let band = band_size(snap, &packets);
            match mp4(&packets.iter().map(|p| (p.data.as_slice(), p.built_ns)).collect::<Vec<_>>(), band, hevc) {
                Ok(bytes) => {
                    let path = PathBuf::from(format!("{}-s{s}.mp4", base.display()));
                    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                    files.push(path);
                }
                Err(e) => set(format!("mp4 of stream {s}: {e}")),
            }
        }
    }
    let tsv = PathBuf::from(format!("{}.tsv", base.display()));
    std::fs::write(&tsv, frame_table(snap, sync.min(from), to)).map_err(|e| format!("{}: {e}", tsv.display()))?;
    let md = PathBuf::from(format!("{}.md", base.display()));
    std::fs::write(&md, notes(req, from, to, sync, (p0, p1, pngs), &png_dir, &files, &tsv)).map_err(|e| format!("{}: {e}", md.display()))?;
    Ok(md)
}

/// A stream's picture size: as its live decoder had it (a tile's own size), else the stream's.
fn band_size(snap: &Snapshot, packets: &[&crate::history::HistPacket]) -> [u32; 2] {
    packets.iter().map(|p| p.size).find(|s| s[0] > 0).unwrap_or(snap.stream_size)
}

/// Decodes `sync..=p1` on CPU decoders, writes `p0..=p1` as PNGs; returns how many. The PNG
/// encodes (the slow part: about a second per 4K picture) run on worker threads.
fn write_pngs(snap: &Snapshot, sync: usize, p0: usize, p1: usize, dir: &Path, set: &dyn Fn(String)) -> Result<usize, String> {
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 6);
    let written = std::sync::atomic::AtomicUsize::new(0);
    let failure: Mutex<Option<String>> = Mutex::new(None);
    // bounded: at most `workers` composed pictures wait (each 33 MB at 4K)
    let (tx, rx) = std::sync::mpsc::sync_channel::<(PathBuf, u32, u32, Vec<u8>)>(workers);
    let rx = Mutex::new(rx);
    std::thread::scope(|scope| -> Result<(), String> {
        let tx = tx;
        for _ in 0..workers {
            let (rx, written, failure) = (&rx, &written, &failure);
            scope.spawn(move || loop {
                let job = rx.lock().unwrap().recv();
                let Ok((path, w, h, rgba)) = job else { return };
                let out = makepad_widgets::Cx::encode_rgba_as_png(w, h, &rgba).and_then(|png| std::fs::write(&path, png).map_err(|e| format!("{}: {e}", path.display())));
                match out {
                    Ok(()) => {
                        written.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Err(e) => *failure.lock().unwrap() = Some(e),
                }
            });
        }
        // per stream: its decoder and the codec it decodes (a key frame in another codec gets a new one)
        let mut decoders: Vec<Option<(VideoStreamDecoder, u8)>> = Vec::new();
        for i in sync..=p1 {
            let mut parts = Vec::new();
            for p in snap.packets_of(i) {
                let s = p.stream as usize;
                if decoders.len() <= s {
                    decoders.resize_with(s + 1, || None);
                }
                if decoders[s].as_ref().is_none_or(|d| d.1 != p.codec) {
                    decoders[s] = Some((VideoStreamDecoder::new(p.video_codec()).map_err(|e| format!("decoder: {e:?}"))?, p.codec));
                }
                for f in decoders[s].as_mut().unwrap().0.push_packet(&p.data, p.frame as i64).map_err(|e| format!("decode of frame {}: {e:?}", p.frame))? {
                    parts.push(((p.x0, p.y0), f));
                }
            }
            if i < p0 {
                continue;
            }
            set(format!("saving PNG {} of {}", i - p0 + 1, p1 - p0 + 1));
            if parts.is_empty() {
                continue;
            }
            // the tiles in their columns and bands
            let width = parts.iter().map(|((x0, _), f)| (x0 + f.width) as usize).max().unwrap_or(0);
            let height = parts.iter().map(|((_, y0), f)| (y0 + f.height) as usize).max().unwrap_or(0);
            let mut rgba = vec![255u8; width * height * 4];
            for ((x0, y0), f) in &parts {
                let rgb = f.to_rgb8();
                let (fw, x0) = (f.width as usize, *x0 as usize);
                for (row, line) in rgb.chunks_exact(fw * 3).enumerate() {
                    let y = row + *y0 as usize;
                    if y >= height {
                        break;
                    }
                    let out = &mut rgba[(y * width + x0) * 4..(y * width + x0 + fw.min(width - x0)) * 4];
                    for (o, px) in out.chunks_exact_mut(4).zip(line.chunks_exact(3)) {
                        o[..3].copy_from_slice(px);
                    }
                }
            }
            let path = dir.join(format!("f{}.png", snap.pictures[i].frame));
            if tx.send((path, width as u32, height as u32, rgba)).is_err() {
                break;
            }
        }
        drop(tx);
        Ok(())
    })?;
    if let Some(e) = failure.into_inner().unwrap() {
        return Err(e);
    }
    Ok(written.into_inner())
}

fn ms(ns: u64) -> f64 {
    ns as f64 / 1e6
}

/// One line per picture, tab separated: what the notes summarise.
fn frame_table(snap: &Snapshot, from: usize, to: usize) -> String {
    let mut out = String::from("frame\tindex\tt_host_ms\tdt_host_ms\tdt_recv_ms\tbytes\tparts\tkey\tencode_ms\tdecode_ms\tmarks\n");
    let t0 = snap.pictures[from].built_ns;
    for i in from..=to {
        let p = &snap.pictures[i];
        let dr = if i > 0 { p.received_ns.saturating_sub(snap.pictures[i - 1].received_ns) } else { 0 };
        let _ = writeln!(
            out,
            "{}\t{i}\t{:.3}\t{:.3}\t{:.3}\t{}\t{}\t{}\t{:.2}\t{:.2}\t{}",
            p.frame,
            ms(p.built_ns.saturating_sub(t0)),
            ms(snap.dt_ns(i)),
            ms(dr),
            p.bytes,
            p.count,
            p.key as u8,
            ms(snap.packets_of(i).iter().map(|q| q.encoded_ns.saturating_sub(q.built_ns)).max().unwrap_or(0)),
            p.decode_us as f64 / 1000.0,
            snap.marks(i).join(", ")
        );
    }
    out
}

fn notes(req: &ClipRequest, from: usize, to: usize, sync: usize, pngs: (usize, usize, usize), png_dir: &Path, files: &[PathBuf], tsv: &Path) -> String {
    let snap = &req.snap;
    let (f0, f1) = (snap.pictures[from].frame, snap.pictures[to].frame);
    let span = snap.pictures[to].built_ns.saturating_sub(snap.pictures[from].built_ns);
    let marked: Vec<(usize, Vec<String>)> = (from..=to).map(|i| (i, snap.marks(i))).filter(|(_, m)| !m.is_empty()).collect();
    let count = |what: &str| marked.iter().filter(|(_, m)| m.iter().any(|x| x.starts_with(what))).count();
    let mut o = String::new();
    let _ = writeln!(o, "# Desk Link clip: {} frames {f0}..{f1}", req.host);
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "{} pictures over {:.1} ms (host clock); shown when saved: frame {}. Unusual: {} gaps, {} frame-number skips, {} size spikes, {} key frames, {} failed decodes (usual interval {:.2} ms, usual picture {:.1} KB).",
        to - from + 1,
        ms(span),
        snap.pictures[req.shown.min(snap.len() - 1)].frame,
        count("gap"),
        marked.iter().filter(|(_, m)| m.iter().any(|x| x.contains("skipped"))).count(),
        count("size"),
        count("key"),
        count("decode failed"),
        ms(snap.median_dt_ns),
        snap.median_bytes as f64 / 1024.0
    );
    let _ = writeln!(o);
    if !marked.is_empty() {
        let _ = writeln!(o, "| frame | dt ms | KB | marks |");
        let _ = writeln!(o, "|---|---|---|---|");
        for (i, m) in marked.iter().take(200) {
            let p = &snap.pictures[*i];
            let _ = writeln!(o, "| {} | {:.2} | {:.1} | {} |", p.frame, ms(snap.dt_ns(*i)), p.bytes as f64 / 1024.0, m.join(", "));
        }
        if marked.len() > 200 {
            let _ = writeln!(o, "\n({} more marked pictures in the .tsv)", marked.len() - 200);
        }
        let _ = writeln!(o);
    }
    let _ = writeln!(o, "Files:");
    let (p0, p1, n) = pngs;
    let _ = writeln!(o, "- PNGs: {} (f<frame>.png, frames {}..{}, {n} written; CPU decode, 4:2:0 colour)", png_dir.display(), snap.pictures[p0].frame, snap.pictures[p1].frame);
    for f in files {
        let _ = writeln!(o, "- {}", f.display());
    }
    if !files.is_empty() {
        let _ = writeln!(
            o,
            "  (bitstream from the key run at frame {}: {} pictures before frame {f0}; one file per stream: tiles (columns and bands) or interleaved engines, in stream order; `makepad-app-desklink --dump-play <s0.h264>,<s1.h264>` plays band tiles; .h265 for H.265)",
            snap.pictures[sync].frame,
            from - sync
        );
    }
    let _ = writeln!(o, "- every picture: {} (frame, index, host time, intervals, bytes, parts, key, host encode ms, live decode ms, marks)", tsv.display());
    let [w, h] = snap.stream_size;
    let _ = writeln!(o, "\nStream {w}x{h}; the history held {} pictures.", snap.len());
    o
}

/// `YYYYmmdd-HHMMSSZ` (UTC), so the files sort by when they were saved.
fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // days since 1970-01-01 -> civil date (Howard Hinnant's algorithm)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

// ---- MP4 (ISO BMFF) of an H.264 or H.265 stream, written as received ----

fn atom(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(body);
    out
}

fn full_atom(name: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut inner = vec![version];
    inner.extend_from_slice(&flags.to_be_bytes()[1..]);
    inner.extend_from_slice(body);
    atom(name, &inner)
}

/// Exp-Golomb reader over an RBSP (emulation prevention removed).
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let b = self.data.get(self.pos / 8)?;
        let v = (b >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(v as u32)
    }

    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let mut v = 0u32;
        for _ in 0..zeros {
            v = (v << 1) | self.bit()?;
        }
        Some((1u32 << zeros) - 1 + v)
    }
}

fn rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// `avcC` from the SPS and PPS (NAL units without start codes).
fn avcc(sps: &[u8], pps: &[u8]) -> Result<Vec<u8>, String> {
    if sps.len() < 4 {
        return Err("short SPS".into());
    }
    let (profile, compat, level) = (sps[1], sps[2], sps[3]);
    let mut out = vec![1, profile, compat, level, 0xFF, 0xE1];
    out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    out.extend_from_slice(sps);
    out.push(1);
    out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    out.extend_from_slice(pps);
    if !matches!(profile, 66 | 77 | 88) {
        // the high profiles say their chroma format and bit depths (High 4:4:4: chroma 3)
        let body = rbsp(&sps[4..]);
        let mut b = Bits { data: &body, pos: 0 };
        let parsed = (|| {
            b.ue()?;
            let chroma = b.ue()?;
            if chroma == 3 {
                b.bit()?;
            }
            Some((chroma, b.ue()?, b.ue()?))
        })();
        let (chroma, luma_depth, chroma_depth) = parsed.ok_or("unreadable SPS")?;
        out.extend_from_slice(&[0xFC | chroma as u8, 0xF8 | luma_depth as u8, 0xF8 | chroma_depth as u8, 0]);
    }
    Ok(out)
}

/// An MP4 of Annex-B access units (`(unit, host time ns)`, the first a key frame with its
/// parameter sets), samples length-prefixed, the parameter sets in the sample entry (`avcC`, or
/// `hvcC` for `hevc`). Stops before a changed SPS (a new stream size: another file's worth).
pub fn mp4(units: &[(&[u8], u64)], size: [u32; 2], hevc: bool) -> Result<Vec<u8>, String> {
    const TIMESCALE: u32 = 90_000;
    // the NAL type: H.264 SPS 7, PPS 8, IDR 5; H.265 VPS 32, SPS 33, PPS 34, IRAP 16..=21
    let (ty, sps_ty, pps_ty): (fn(&[u8]) -> u8, u8, u8) = if hevc { (annex_b::hevc_nal_unit_type, 33, 34) } else { (annex_b::nal_unit_type, 7, 8) };
    let is_key = |t: u8| if hevc { (16..=21).contains(&t) } else { t == 5 };
    let mut sps: Option<Vec<u8>> = None;
    let mut pps: Option<Vec<u8>> = None;
    let mut vps: Option<Vec<u8>> = None;
    let mut samples: Vec<(Vec<u8>, u64, bool)> = Vec::new();
    'units: for (unit, t) in units {
        let mut nals = Vec::new();
        let mut key = false;
        for nal in annex_b::split_annex_b(unit) {
            match ty(nal) {
                t if t == sps_ty => {
                    if sps.as_deref().is_some_and(|s| s != nal) {
                        break 'units;
                    }
                    sps = Some(nal.to_vec());
                }
                t if t == pps_ty => pps = pps.or_else(|| Some(nal.to_vec())),
                32 if hevc => vps = vps.or_else(|| Some(nal.to_vec())),
                t => {
                    key |= is_key(t);
                    nals.push(nal);
                }
            }
        }
        if nals.is_empty() {
            continue;
        }
        samples.push((annex_b::annex_b_to_avcc(&nals), *t, key));
    }
    let (Some(sps), Some(pps)) = (sps, pps) else { return Err("no SPS/PPS".into()) };
    if samples.is_empty() {
        return Err("no pictures".into());
    }
    let config = if hevc {
        let vps = vps.ok_or("no VPS")?;
        let mut sets = Vec::new();
        for nal in [&vps, &sps, &pps] {
            sets.extend_from_slice(&[0, 0, 0, 1]);
            sets.extend_from_slice(nal);
        }
        hevc_config::hvcc_from_annex_b(&sets).ok_or("unreadable SPS")?
    } else {
        avcc(&sps, &pps)?
    };
    let (entry, config_box, brand) = if hevc { (b"hvc1", b"hvcC", b"hvc1") } else { (b"avc1", b"avcC", b"avc1") };
    // durations from the host's times; the last one the one before it
    let mut deltas: Vec<u32> = samples.windows(2).map(|w| ((w[1].1.saturating_sub(w[0].1)) as u128 * TIMESCALE as u128 / 1_000_000_000).max(1) as u32).collect();
    deltas.push(deltas.last().copied().unwrap_or(TIMESCALE / 60));
    let duration: u64 = deltas.iter().map(|d| *d as u64).sum();

    let ftyp = {
        let mut b = b"isom".to_vec();
        b.extend_from_slice(&512u32.to_be_bytes());
        for brand in [b"isom", b"iso2", brand, b"mp41"] {
            b.extend_from_slice(brand);
        }
        atom(b"ftyp", &b)
    };
    let mdat_len: usize = samples.iter().map(|s| s.0.len()).sum();
    let first_offset = (ftyp.len() + 8) as u32;
    if mdat_len as u64 + 8 + ftyp.len() as u64 > u32::MAX as u64 {
        return Err("clip over 4 GB".into());
    }
    let identity: Vec<u8> = [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000].iter().flat_map(|v| v.to_be_bytes()).collect();
    let stsd = {
        let mut e = vec![0u8; 6];
        e.extend_from_slice(&1u16.to_be_bytes());
        e.extend_from_slice(&[0u8; 16]);
        e.extend_from_slice(&(size[0] as u16).to_be_bytes());
        e.extend_from_slice(&(size[1] as u16).to_be_bytes());
        e.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        e.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        e.extend_from_slice(&[0u8; 4]);
        e.extend_from_slice(&1u16.to_be_bytes());
        e.extend_from_slice(&[0u8; 32]);
        e.extend_from_slice(&0x0018u16.to_be_bytes());
        e.extend_from_slice(&0xFFFFu16.to_be_bytes());
        e.extend_from_slice(&atom(config_box, &config));
        let mut b = 1u32.to_be_bytes().to_vec();
        b.extend_from_slice(&atom(entry, &e));
        full_atom(b"stsd", 0, 0, &b)
    };
    let stts = {
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for d in &deltas {
            match runs.last_mut() {
                Some((n, v)) if *v == *d => *n += 1,
                _ => runs.push((1, *d)),
            }
        }
        let mut b = (runs.len() as u32).to_be_bytes().to_vec();
        for (n, v) in runs {
            b.extend_from_slice(&n.to_be_bytes());
            b.extend_from_slice(&v.to_be_bytes());
        }
        full_atom(b"stts", 0, 0, &b)
    };
    let stss = {
        let keys: Vec<u32> = samples.iter().enumerate().filter(|(_, s)| s.2).map(|(i, _)| i as u32 + 1).collect();
        let mut b = (keys.len() as u32).to_be_bytes().to_vec();
        for k in keys {
            b.extend_from_slice(&k.to_be_bytes());
        }
        full_atom(b"stss", 0, 0, &b)
    };
    let stsc = {
        let mut b = 1u32.to_be_bytes().to_vec();
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&(samples.len() as u32).to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        full_atom(b"stsc", 0, 0, &b)
    };
    let stsz = {
        let mut b = 0u32.to_be_bytes().to_vec();
        b.extend_from_slice(&(samples.len() as u32).to_be_bytes());
        for s in &samples {
            b.extend_from_slice(&(s.0.len() as u32).to_be_bytes());
        }
        full_atom(b"stsz", 0, 0, &b)
    };
    let stco = {
        let mut b = 1u32.to_be_bytes().to_vec();
        b.extend_from_slice(&first_offset.to_be_bytes());
        full_atom(b"stco", 0, 0, &b)
    };
    let stbl = atom(b"stbl", &[stsd, stts, stss, stsc, stsz, stco].concat());
    let vmhd = full_atom(b"vmhd", 0, 1, &[0u8; 8]);
    let dinf = {
        let mut b = 1u32.to_be_bytes().to_vec();
        b.extend_from_slice(&full_atom(b"url ", 0, 1, &[]));
        atom(b"dinf", &full_atom(b"dref", 0, 0, &b))
    };
    let minf = atom(b"minf", &[vmhd, dinf, stbl].concat());
    let mdhd = {
        let mut b = vec![0u8; 8];
        b.extend_from_slice(&TIMESCALE.to_be_bytes());
        b.extend_from_slice(&(duration.min(u32::MAX as u64) as u32).to_be_bytes());
        b.extend_from_slice(&0x55C4u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        full_atom(b"mdhd", 0, 0, &b)
    };
    let hdlr = {
        let mut b = vec![0u8; 4];
        b.extend_from_slice(b"vide");
        b.extend_from_slice(&[0u8; 12]);
        b.extend_from_slice(b"VideoHandler\0");
        full_atom(b"hdlr", 0, 0, &b)
    };
    let mdia = atom(b"mdia", &[mdhd, hdlr, minf].concat());
    let tkhd = {
        let mut b = vec![0u8; 8];
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&[0u8; 4]);
        b.extend_from_slice(&(duration.min(u32::MAX as u64) as u32).to_be_bytes());
        b.extend_from_slice(&[0u8; 16]);
        b.extend_from_slice(&identity);
        b.extend_from_slice(&(size[0] << 16).to_be_bytes());
        b.extend_from_slice(&(size[1] << 16).to_be_bytes());
        full_atom(b"tkhd", 0, 7, &b)
    };
    let trak = atom(b"trak", &[tkhd, mdia].concat());
    let mvhd = {
        let mut b = vec![0u8; 8];
        b.extend_from_slice(&TIMESCALE.to_be_bytes());
        b.extend_from_slice(&(duration.min(u32::MAX as u64) as u32).to_be_bytes());
        b.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        b.extend_from_slice(&0x0100u16.to_be_bytes());
        b.extend_from_slice(&[0u8; 10]);
        b.extend_from_slice(&identity);
        b.extend_from_slice(&[0u8; 24]);
        b.extend_from_slice(&2u32.to_be_bytes());
        full_atom(b"mvhd", 0, 0, &b)
    };
    let moov = atom(b"moov", &[mvhd, trak].concat());
    let mut out = Vec::with_capacity(ftyp.len() + 8 + mdat_len + moov.len());
    out.extend_from_slice(&ftyp);
    out.extend_from_slice(&((mdat_len + 8) as u32).to_be_bytes());
    out.extend_from_slice(b"mdat");
    for s in &samples {
        out.extend_from_slice(&s.0);
    }
    out.extend_from_slice(&moov);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walks the boxes of `data`, returning (name, body) pairs at this level.
    fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
        let mut out = Vec::new();
        let mut i = 0;
        while i + 8 <= data.len() {
            let len = u32::from_be_bytes(data[i..i + 4].try_into().unwrap()) as usize;
            out.push((data[i + 4..i + 8].try_into().unwrap(), &data[i + 8..i + len]));
            i += len;
        }
        assert_eq!(i, data.len());
        out
    }

    #[test]
    fn avcc_of_a_444_sps_names_its_chroma() {
        // profile 244, level 51, sps_id 0 (1), chroma_format_idc 3 (00100), separate planes 0,
        // luma depth 0 (1), chroma depth 0 (1): bits 1 00100 0 1 1 -> 1001 0001 1...
        let sps = [0x67, 244, 0, 51, 0b1001_0001, 0b1000_0000];
        let c = avcc(&sps, &[0x68, 0xEE]).unwrap();
        assert_eq!(&c[..6], &[1, 244, 0, 51, 0xFF, 0xE1]);
        assert_eq!(&c[c.len() - 4..], &[0xFC | 3, 0xF8, 0xF8, 0]);
        // main profile: no extension
        let c = avcc(&[0x67, 77, 0, 40, 0x80], &[0x68, 0xEE]).unwrap();
        assert_eq!(c.len(), 6 + 2 + 5 + 1 + 2 + 2);
    }

    #[test]
    fn mp4_holds_every_picture_with_its_key_frames() {
        let key = [&[0u8, 0, 0, 1, 0x67, 77, 0, 40, 0x80][..], &[0, 0, 0, 1, 0x68, 0xEE], &[0, 0, 0, 1, 0x65, 1, 2, 3]].concat();
        let p = [0u8, 0, 0, 1, 0x41, 9, 9].to_vec();
        let units: Vec<(&[u8], u64)> = vec![(&key, 0), (&p, 8_333_333), (&p, 16_666_666), (&key, 25_000_000)];
        let file = mp4(&units, [640, 360], false).unwrap();
        let top = boxes(&file);
        assert_eq!(top.iter().map(|b| b.0).collect::<Vec<_>>(), vec![*b"ftyp", *b"mdat", *b"moov"]);
        // samples: length-prefixed, no parameter sets: 4+4, 4+3, 4+3, 4+4
        assert_eq!(top[1].1.len(), 8 + 7 + 7 + 8);
        let moov = boxes(top[2].1);
        let trak = boxes(moov[1].1);
        let mdia = boxes(trak[1].1);
        let minf = boxes(mdia[2].1);
        let stbl = boxes(minf[2].1);
        let names: Vec<[u8; 4]> = stbl.iter().map(|b| b.0).collect();
        assert_eq!(names, vec![*b"stsd", *b"stts", *b"stss", *b"stsc", *b"stsz", *b"stco"]);
        // sync samples 1 and 4
        assert_eq!(&stbl[2].1[4..], &[0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 4]);
        // the chunk starts right after mdat's header
        assert_eq!(u32::from_be_bytes(stbl[5].1[8..12].try_into().unwrap()) as usize, top[0].1.len() + 8 + 8);
    }

    /// With `DESKLINK_MP4_IN=<dump.h264> DESKLINK_MP4_OUT=<out.mp4> DESKLINK_MP4_SIZE=<w>x<h>`: the
    /// dump as an MP4 (for a check with a player); without them nothing happens.
    #[test]
    fn mp4_of_a_dump() {
        let (Ok(input), Ok(out), Ok(size)) = (std::env::var("DESKLINK_MP4_IN"), std::env::var("DESKLINK_MP4_OUT"), std::env::var("DESKLINK_MP4_SIZE")) else { return };
        let (w, h) = size.split_once('x').unwrap();
        let data = std::fs::read(input).unwrap();
        let mut units = Vec::new();
        let mut unit = Vec::new();
        for nal in annex_b::split_annex_b(&data) {
            unit.extend_from_slice(&[0, 0, 0, 1]);
            unit.extend_from_slice(nal);
            if matches!(annex_b::nal_unit_type(nal), 1 | 5) {
                units.push(std::mem::take(&mut unit));
            }
        }
        let timed: Vec<(&[u8], u64)> = units.iter().enumerate().map(|(i, u)| (u.as_slice(), i as u64 * 8_333_333)).collect();
        std::fs::write(out, mp4(&timed, [w.parse().unwrap(), h.parse().unwrap()], false).unwrap()).unwrap();
    }

    #[test]
    fn stamp_is_a_utc_date() {
        let s = utc_stamp();
        assert_eq!(s.len(), 16);
        assert!(s.starts_with("20"));
        assert!(s.ends_with('Z'));
    }
}
