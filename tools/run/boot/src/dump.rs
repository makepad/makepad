//! `MAKEPAD_RUN_BOOT_DUMP=<dir>[@<ms>]`: the boot window's frames as PNG files, for looking at a
//! hidden run (the HAL's snapshot of the presented window, answered by a `Readback` event). While
//! the JIT activity badge shows, a frame is the app's window with the badge put over it (two
//! snapshots, composed here).
use makepad_hal::{format, usage, Attr, CmdWriter, Id};
use std::{path::PathBuf, time::Instant};

/// The texture the field is drawn into after the window was handed to the app.
pub const TARGET: Id = Id::new(7, 1);

pub struct Dump {
    dir: PathBuf,
    every_ms: u128,
    began: Instant,
    last: Option<u128>,
    next: u32,
    pending: Vec<(u32, u128)>,
    /// the app's window and the activity badge over it: read back apart, written as one frame
    pairs: Vec<Pair>,
    target: Option<[u32; 2]>,
    /// `MAKEPAD_RUN_BOOT_HOLD=1`: keep drawing (into a texture) after the app has the window
    pub hold: bool,
    /// `MAKEPAD_RUN_BOOT_CHROME=client|<x>,<y>,<w>,<h>`: the window chrome a hidden run (which has
    /// no window system, so no window buttons) lays its caption out for: app-drawn buttons, or
    /// native ones at that rect (points)
    pub chrome: Option<crate::caption::Chrome>,
}

impl Dump {
    pub fn from_env() -> Option<Self> {
        let spec = std::env::var("MAKEPAD_RUN_BOOT_DUMP").ok()?;
        let (dir, every) = match spec.rsplit_once('@') {
            Some((dir, ms)) => (dir, ms.parse().unwrap_or(250)),
            None => (spec.as_str(), 250),
        };
        std::fs::create_dir_all(dir).ok()?;
        Some(Self {
            dir: dir.into(),
            every_ms: every,
            began: Instant::now(),
            last: None,
            next: 1,
            pending: Vec::new(),
            pairs: Vec::new(),
            target: None,
            hold: std::env::var_os("MAKEPAD_RUN_BOOT_HOLD").is_some(),
            chrome: std::env::var("MAKEPAD_RUN_BOOT_CHROME").ok().and_then(|spec| {
                if spec == "client" {
                    return Some(crate::caption::Chrome { client: true, ..Default::default() });
                }
                let v: Vec<f32> = spec.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                let native: [f32; 4] = v.try_into().ok()?;
                Some(crate::caption::Chrome { native, client: false, state: 0 })
            }),
        })
    }

    /// The request number of a snapshot to take with this frame, if one is due.
    pub fn due(&mut self) -> Option<u32> {
        let ms = self.began.elapsed().as_millis();
        if self.last.is_some_and(|last| ms < last + self.every_ms) {
            return None;
        }
        self.now()
    }

    /// A snapshot of this frame whatever the interval: the halted view draws only when it
    /// changes, so each of its frames is kept.
    pub fn now(&mut self) -> Option<u32> {
        let ms = self.began.elapsed().as_millis();
        if self.pending.len() + self.pairs.len() > 4 {
            return None;
        }
        self.last = Some(ms);
        let req = self.next;
        self.next += 1;
        self.pending.push((req, ms));
        Some(req)
    }

    /// The offscreen target for a frame of `size` pixels (created or made again as needed).
    pub fn target(&mut self, writer: &mut CmdWriter, size: [u32; 2]) -> Id {
        if self.target != Some(size) {
            if self.target.is_some() {
                writer.texture_destroy(TARGET);
            }
            writer.texture_create(
                TARGET,
                usage::RENDER_TARGET,
                format::BGRA8,
                size[0],
                size[1],
                [1, 1],
                Attr::default(),
            );
            self.target = Some(size);
        }
        TARGET
    }

    /// ms on the dump's clock (its frames' file names)
    pub fn ms(&self) -> u128 {
        self.began.elapsed().as_millis()
    }

    /// The snapshot `due` just asked for is the app's window: this one, of the activity badge
    /// placed at `placed` (window points), goes over it.
    pub fn badge(&mut self, placed: [f32; 4], dpi: f32) -> u32 {
        let (app, ms) = self.pending.pop().unwrap_or((0, 0));
        let req = self.next;
        self.next += 1;
        let at = [(placed[0] * dpi).round() as i64, (placed[1] * dpi).round() as i64];
        self.pairs.push(Pair { app, badge: req, ms, at, app_px: None, badge_px: None });
        req
    }

    pub fn ready(&mut self, req: u32, rect: [u32; 4], pixels: &[u8]) {
        if let Some(at) = self.pairs.iter().position(|p| p.app == req || p.badge == req) {
            let [_, _, w, h] = rect;
            if pixels.len() < (w * h * 4) as usize || w == 0 || h == 0 {
                self.pairs.remove(at);
                return;
            }
            let pair = &mut self.pairs[at];
            let image = Some((w, h, pixels[..(w * h * 4) as usize].to_vec()));
            if pair.app == req {
                pair.app_px = image;
            } else {
                pair.badge_px = image;
            }
            if pair.app_px.is_some() && pair.badge_px.is_some() {
                let pair = self.pairs.remove(at);
                let (w, h, mut app) = pair.app_px.unwrap();
                let (bw, bh, badge) = pair.badge_px.unwrap();
                // premultiplied over
                for y in 0..bh as i64 {
                    for x in 0..bw as i64 {
                        let (ax, ay) = (pair.at[0] + x, pair.at[1] + y);
                        if ax < 0 || ay < 0 || ax >= w as i64 || ay >= h as i64 {
                            continue;
                        }
                        let b = ((y * bw as i64 + x) * 4) as usize;
                        let a = ((ay * w as i64 + ax) * 4) as usize;
                        let alpha = badge[b + 3] as u32;
                        for c in 0..3 {
                            app[a + c] = (badge[b + c] as u32 + app[a + c] as u32 * (255 - alpha) / 255).min(255) as u8;
                        }
                    }
                }
                let path = self.dir.join(format!("frame-{:06}.png", pair.ms));
                let _ = std::fs::write(path, png(w, h, &app));
            }
            return;
        }
        let Some(at) = self.pending.iter().position(|p| p.0 == req) else {
            return;
        };
        let (_, ms) = self.pending.remove(at);
        let [_, _, w, h] = rect;
        // a window that keeps no frame answers with nothing
        if pixels.len() < (w * h * 4) as usize || w == 0 || h == 0 {
            return;
        }
        let path = self.dir.join(format!("frame-{ms:06}.png"));
        let _ = std::fs::write(path, png(w, h, pixels));
    }
}

struct Pair {
    app: u32,
    badge: u32,
    ms: u128,
    /// the badge's place in the app's image (pixels)
    at: [i64; 2],
    app_px: Option<(u32, u32, Vec<u8>)>,
    badge_px: Option<(u32, u32, Vec<u8>)>,
}

fn crc(mut c: u32, data: &[u8], table: &[u32; 256]) -> u32 {
    for b in data {
        c = table[((c ^ *b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    c
}

/// An uncompressed (stored-block) PNG of RGBA8 rows.
fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut table = [0u32; 256];
    for (n, t) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    let mut raw = Vec::with_capacity((w as usize * 3 + 1) * h as usize);
    for row in rgba.chunks_exact(w as usize * 4).take(h as usize) {
        raw.push(0);
        for px in row.chunks_exact(4) {
            raw.extend_from_slice(&px[..3]);
        }
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(chunk) = chunks.next() {
        z.push(chunks.peek().is_none() as u8);
        z.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(chunk.len() as u16)).to_le_bytes());
        z.extend_from_slice(chunk);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for v in &raw {
        a = (a + *v as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let c = !crc(crc(!0, kind, &table), data, &table);
        out.extend_from_slice(&c.to_be_bytes());
    };
    let mut head = Vec::new();
    head.extend_from_slice(&w.to_be_bytes());
    head.extend_from_slice(&h.to_be_bytes());
    head.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &head);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}
