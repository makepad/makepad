//! `MAKEPAD_RUN_BOOT_DUMP=<dir>[@<ms>]`: the boot window's frames as PNG files, for looking at a
//! hidden run (the HAL's snapshot of the presented window, answered by a `Readback` event).
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
    target: Option<[u32; 2]>,
    /// `MAKEPAD_RUN_BOOT_HOLD=1`: keep drawing (into a texture) after the app has the window
    pub hold: bool,
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
            target: None,
            hold: std::env::var_os("MAKEPAD_RUN_BOOT_HOLD").is_some(),
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
        if self.pending.len() > 4 {
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

    pub fn ready(&mut self, req: u32, rect: [u32; 4], pixels: &[u8]) {
        let Some(at) = self.pending.iter().position(|p| p.0 == req) else {
            return;
        };
        let (_, ms) = self.pending.remove(at);
        let [_, _, w, h] = rect;
        if pixels.len() < (w * h * 4) as usize {
            return;
        }
        let path = self.dir.join(format!("frame-{ms:06}.png"));
        let _ = std::fs::write(path, png(w, h, pixels));
    }
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
