//! The wire of the hub's `kernel` domain: Splash compute kernels run on a
//! node's CPUs for a fleet client (`crate::kernel_compute` is the node
//! side, `crate::kernel_client` the client side).
//!
//! A job is one Splash compute kernel (its source; the node compiles it and
//! caches it by hash), the params and inputs every call shares, and a list
//! of tiles. A tile is one call of the kernel over `count` elements with its
//! own param overrides and inputs (a fractal's view rectangle as an `f64`
//! input, a chunk's offset). Each tile comes back with every buffer the
//! kernel writes (and a reduce kernel's lanes). The kernels are total,
//! bounded and bit-identical on every backend, so a tile rendered on an x64
//! node is the tile an arm64 node or the client itself renders.
//!
//! ```text
//! GET  /kernel                      -> KernelInfoJson (cores, cache, totals)
//! POST /kernel/jobs                 KernelJobJson -> KernelSubmittedJson {job_id, tiles}
//! GET  /kernel/job/<id>/tiles?timeout_ms=N
//!                                   long poll: the tiles finished since the last poll
//!                                   (binary batch below); flag DONE once every tile
//!                                   was sent, after which the job is gone
//! POST /kernel/job/<id>/cancel      queued tiles are dropped, running ones stopped
//! GET  /kernel/stream               websocket: the same submit/cancel, tiles pushed
//!                                   as they finish (frames below)
//! ```
//!
//! Only the credential that submitted a job may poll or cancel it. Jobs
//! nobody polls for [`JOB_IDLE_SECS`] are cancelled and dropped.
//!
//! The batch (little-endian):
//!
//! ```text
//! batch   "MKKB" u32 version  u32 flags (1 = DONE)  u32 tiles  tile*
//! tile    u64 id  u32 status  u32 elements  u64 work  u64 nanos
//!         u32 lanes  f32*lanes (a reduce kernel's result)
//!         u32 outputs  output*  u32 message_len  utf8 message
//! output  u32 name_len  name  u32 encoding  u32 words  u32 bytes  payload
//! ```
//!
//! Encodings: 0 raw words; 1 delta-deflate: each word minus the one before
//! (wrapping), the four byte planes of those one after the other, then raw
//! DEFLATE. Smooth fields (iteration counts, heights) shrink 5-20x.

use makepad_micro_serde::*;

pub const KERNEL_API_VERSION: u32 = 1;
/// The longest kernel source a node accepts.
pub const MAX_SOURCE_BYTES: usize = 64 * 1024;
/// Most tiles in one job.
pub const MAX_TILES: usize = 4096;
/// Most elements in one tile (one call).
pub const MAX_TILE_ELEMENTS: usize = 1 << 22;
/// Most words one input buffer may hold.
pub const MAX_INPUT_WORDS: usize = 1 << 22;
/// Most output words one tile may produce (all its outputs).
pub const MAX_TILE_OUTPUT_WORDS: usize = 1 << 22;
/// Most passes of one tile.
pub const MAX_PASSES: usize = 64;
/// Most output words a whole job may produce.
pub const MAX_JOB_OUTPUT_WORDS: usize = 1 << 26;
/// A job nobody polls for this long is cancelled and dropped.
pub const JOB_IDLE_SECS: u64 = 30;
/// The longest a tiles poll waits for a tile.
pub const MAX_POLL_MS: u64 = 10_000;

pub const ENCODING_RAW: u32 = 0;
pub const ENCODING_DELTA_DEFLATE: u32 = 1;

pub const STATUS_OK: u32 = 0;
pub const STATUS_CANCELLED: u32 = 1;
pub const STATUS_OVER_BUDGET: u32 = 2;
pub const STATUS_FAILED: u32 = 3;

pub const FLAG_DONE: u32 = 1;

// The stream (`GET /kernel/stream`, a websocket: one long-lived TLS stream
// per node, no request per poll). Every frame is binary, its first byte the
// kind, then a little-endian u64:
// client -> node: STREAM_SUBMIT request-id + KernelJobJson;
//                 STREAM_CANCEL job-id.
// node -> client: STREAM_SUBMITTED request-id + u16 status + KernelSubmittedJson
//                 (or the error text); STREAM_BATCH job-id + a tile batch,
//                 pushed as tiles finish (the job is over after the DONE one).
pub const STREAM_SUBMIT: u8 = 1;
pub const STREAM_CANCEL: u8 = 2;
pub const STREAM_SUBMITTED: u8 = 1;
pub const STREAM_BATCH: u8 = 2;

const MAGIC: &[u8; 4] = b"MKKB";

/// A param by name (Splash `param(...)`, f32, clamped to its declared range).
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct KernelParamJson {
    pub name: String,
    pub value: f32,
}

/// A read-only buffer by name: little-endian u32 words, base64. An `f64`
/// buffer is two words per value, low then high.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct KernelBufferJson {
    pub name: String,
    pub words: String,
}

impl KernelBufferJson {
    pub fn from_words(name: &str, words: &[u32]) -> Self {
        let mut bytes = Vec::with_capacity(words.len() * 4);
        for w in words {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        let text = makepad_base64::base64_encode(&bytes, &makepad_base64::BASE64_STANDARD);
        Self { name: name.to_string(), words: String::from_utf8(text).unwrap_or_default() }
    }

    pub fn from_f64(name: &str, values: &[f64]) -> Self {
        let words: Vec<u32> = values.iter().flat_map(|v| {
            let b = v.to_bits();
            [b as u32, (b >> 32) as u32]
        }).collect();
        Self::from_words(name, &words)
    }

    pub fn from_f32(name: &str, values: &[f32]) -> Self {
        let words: Vec<u32> = values.iter().map(|v| v.to_bits()).collect();
        Self::from_words(name, &words)
    }

    /// The words, at most `max_words` of them.
    pub fn decode(&self, max_words: usize) -> Result<Vec<u32>, String> {
        if self.words.len() > max_words.saturating_mul(4).saturating_mul(4) / 3 + 4 {
            return Err(format!("input `{}` is larger than {} words", self.name, max_words));
        }
        let bytes = makepad_base64::base64_decode(self.words.as_bytes()).map_err(|e| format!("input `{}`: bad base64: {e:?}", self.name))?;
        if bytes.len() % 4 != 0 {
            return Err(format!("input `{}`: {} bytes is not whole words", self.name, bytes.len()));
        }
        if bytes.len() / 4 > max_words {
            return Err(format!("input `{}` is larger than {} words", self.name, max_words));
        }
        Ok(bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
    }
}

/// One call of the kernel.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct KernelTileJson {
    /// The caller's id for the tile, returned with its result.
    pub id: u64,
    /// Elements of this call (the kernel's `i` runs 0..count).
    pub count: u32,
    /// Overrides of the job's params for this tile.
    pub params: Vec<KernelParamJson>,
    /// This tile's own read-only buffers (over the job's of the same name).
    pub inputs: Vec<KernelBufferJson>,
}

/// `POST /kernel/jobs`.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct KernelJobJson {
    /// Splash compute kernel source (`platform/script/compute`).
    pub source: String,
    pub params: Vec<KernelParamJson>,
    pub inputs: Vec<KernelBufferJson>,
    /// Tiles in the order the node should run them (nearest first).
    pub tiles: Vec<KernelTileJson>,
    /// "raw" or "delta-deflate" for every output.
    pub encoding: String,
    /// The `seed` input of every call.
    pub seed: u32,
    /// Calls per tile (default 1): a kernel whose work per element would
    /// pass the per-call bound runs it in passes, carrying its state from
    /// one to the next through `feedback`; its `pass` param is the index.
    pub passes: Option<u32>,
    /// (output, input) pairs: after a pass, each output is the input of the
    /// same tile's next pass (zeros for the first). Fed-back outputs stay on
    /// the node; the tile returns the other outputs of the last pass.
    pub feedback: Option<Vec<KernelFeedbackJson>>,
}

#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct KernelFeedbackJson {
    pub output: String,
    pub input: String,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct KernelSubmittedJson {
    pub job_id: u64,
    pub tiles: u32,
    /// The kernel's cache key on the node (sha-256 of the source).
    pub kernel: String,
    /// Whether it compiled now (false: it was cached).
    pub compiled: bool,
}

/// `GET /kernel`.
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct KernelInfoJson {
    pub api: u32,
    /// Worker threads (the node's logical cores unless configured).
    pub cores: u32,
    /// `x86_64`, `aarch64`.
    pub arch: String,
    /// `native` or `interp`.
    pub backend: String,
    /// Kernels in the compile cache.
    pub kernels_cached: u32,
    pub jobs: u32,
    pub tiles_queued: u32,
    pub tiles_done: u64,
    pub elements_done: u64,
    /// Summed worker busy time.
    pub busy_ms: u64,
}

pub fn encoding_of(name: &str) -> Option<u32> {
    match name {
        "raw" | "" => Some(ENCODING_RAW),
        "delta-deflate" => Some(ENCODING_DELTA_DEFLATE),
        _ => None,
    }
}

/// One output buffer of a finished tile.
#[derive(Clone, Debug, PartialEq)]
pub struct TileOutput {
    pub name: String,
    pub words: Vec<u32>,
}

/// One finished tile.
#[derive(Clone, Debug, PartialEq)]
pub struct TileResult {
    pub id: u64,
    pub status: u32,
    pub elements: u32,
    /// Counted ops (the same on every machine).
    pub work: u64,
    /// Wall time on the node's worker.
    pub nanos: u64,
    pub reduced: Vec<f32>,
    pub outputs: Vec<TileOutput>,
    pub message: String,
}

impl TileResult {
    pub fn output(&self, name: &str) -> Option<&[u32]> {
        self.outputs.iter().find(|o| o.name == name).map(|o| o.words.as_slice())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileBatch {
    /// Every tile of the job has been sent (this batch included).
    pub done: bool,
    pub tiles: Vec<TileResult>,
}

/// Encodes one tile's record (appended to a batch by [`batch_bytes`]).
pub fn encode_tile(tile: &TileResult, encoding: u32, out: &mut Vec<u8>) {
    put64(out, tile.id);
    put32(out, tile.status);
    put32(out, tile.elements);
    put64(out, tile.work);
    put64(out, tile.nanos);
    put32(out, tile.reduced.len() as u32);
    for r in &tile.reduced {
        put32(out, r.to_bits());
    }
    put32(out, tile.outputs.len() as u32);
    for o in &tile.outputs {
        put32(out, o.name.len() as u32);
        out.extend_from_slice(o.name.as_bytes());
        let payload = encode_words(&o.words, encoding);
        put32(out, encoding);
        put32(out, o.words.len() as u32);
        put32(out, payload.len() as u32);
        out.extend_from_slice(&payload);
    }
    put32(out, tile.message.len() as u32);
    out.extend_from_slice(tile.message.as_bytes());
}

/// A batch from tile records made by [`encode_tile`].
pub fn batch_bytes(done: bool, tiles: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + tiles.iter().map(|t| t.len()).sum::<usize>());
    out.extend_from_slice(MAGIC);
    put32(&mut out, KERNEL_API_VERSION);
    put32(&mut out, if done { FLAG_DONE } else { 0 });
    put32(&mut out, tiles.len() as u32);
    for t in tiles {
        out.extend_from_slice(t);
    }
    out
}

/// Decodes a batch; no output may hold more than `max_words` words.
pub fn decode_batch(bytes: &[u8], max_words: usize) -> Result<TileBatch, String> {
    let mut r = Reader { b: bytes, at: 0 };
    if r.take(4)? != MAGIC {
        return Err("not a kernel tile batch".into());
    }
    let version = r.u32()?;
    if version != KERNEL_API_VERSION {
        return Err(format!("tile batch version {version}, expected {KERNEL_API_VERSION}"));
    }
    let flags = r.u32()?;
    let n = r.u32()? as usize;
    if n > MAX_TILES {
        return Err(format!("{n} tiles in one batch"));
    }
    let mut tiles = Vec::with_capacity(n);
    for _ in 0..n {
        let id = r.u64()?;
        let status = r.u32()?;
        let elements = r.u32()?;
        let work = r.u64()?;
        let nanos = r.u64()?;
        let lanes = r.u32()? as usize;
        if lanes > 16 {
            return Err(format!("{lanes} reduce lanes"));
        }
        let mut reduced = Vec::with_capacity(lanes);
        for _ in 0..lanes {
            reduced.push(f32::from_bits(r.u32()?));
        }
        let n_out = r.u32()? as usize;
        if n_out > 64 {
            return Err(format!("{n_out} outputs"));
        }
        let mut outputs = Vec::with_capacity(n_out);
        for _ in 0..n_out {
            let name_len = r.u32()? as usize;
            let name = String::from_utf8(r.take(name_len)?.to_vec()).map_err(|_| "output name is not utf-8".to_string())?;
            let encoding = r.u32()?;
            let words = r.u32()? as usize;
            let len = r.u32()? as usize;
            if words > max_words {
                return Err(format!("output `{name}` has {words} words; at most {max_words} expected"));
            }
            let payload = r.take(len)?;
            outputs.push(TileOutput { name, words: decode_words(payload, encoding, words)? });
        }
        let msg_len = r.u32()? as usize;
        let message = String::from_utf8_lossy(r.take(msg_len)?).into_owned();
        tiles.push(TileResult { id, status, elements, work, nanos, reduced, outputs, message });
    }
    if r.at != bytes.len() {
        return Err("trailing bytes after the tile batch".into());
    }
    Ok(TileBatch { done: flags & FLAG_DONE != 0, tiles })
}

/// Words in an encoding.
pub fn encode_words(words: &[u32], encoding: u32) -> Vec<u8> {
    match encoding {
        ENCODING_DELTA_DEFLATE => {
            let n = words.len();
            let mut planes = vec![0u8; n * 4];
            let mut prev = 0u32;
            for (i, w) in words.iter().enumerate() {
                let d = w.wrapping_sub(prev).to_le_bytes();
                prev = *w;
                planes[i] = d[0];
                planes[n + i] = d[1];
                planes[2 * n + i] = d[2];
                planes[3 * n + i] = d[3];
            }
            makepad_fast_inflate::deflate_compress(&planes, 1)
        }
        _ => {
            let mut out = Vec::with_capacity(words.len() * 4);
            for w in words {
                out.extend_from_slice(&w.to_le_bytes());
            }
            out
        }
    }
}

/// `words` words back from an encoding (never more).
pub fn decode_words(payload: &[u8], encoding: u32, words: usize) -> Result<Vec<u32>, String> {
    match encoding {
        ENCODING_RAW => {
            if payload.len() != words * 4 {
                return Err(format!("raw output of {} bytes for {words} words", payload.len()));
            }
            Ok(payload.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
        }
        ENCODING_DELTA_DEFLATE => {
            let mut planes = vec![0u8; words * 4];
            let (_, written) = makepad_fast_inflate::deflate_decompress(payload, &mut planes).map_err(|e| format!("delta-deflate: {e:?}"))?;
            if written != planes.len() {
                return Err(format!("delta-deflate gave {written} bytes for {words} words"));
            }
            let n = words;
            let mut out = Vec::with_capacity(n);
            let mut prev = 0u32;
            for i in 0..n {
                let d = u32::from_le_bytes([planes[i], planes[n + i], planes[2 * n + i], planes[3 * n + i]]);
                prev = prev.wrapping_add(d);
                out.push(prev);
            }
            Ok(out)
        }
        other => Err(format!("unknown output encoding {other}")),
    }
}

/// Kernels shipped with the hub: its first client's (the fractal tile
/// renderer), used by the hub's tests and `kernel-bench` too.
pub mod kernels {
    use super::*;

    /// Escape-time fractals in f64, one res x res tile per call (see the
    /// file's header for the texel format): Mandelbrot, Julia, Burning
    /// Ship, Tricorn.
    pub const FRACTAL: &str = include_str!("kernels/fractal.splash");
    /// Multibrot (z^power + c), the same format.
    pub const MULTIBROT: &str = include_str!("kernels/multibrot.splash");
    /// Mandelbrot..Multibrot past one call's iteration bound: tiles in
    /// passes of 2048 iterations, the orbit state fed back between them.
    pub const FRACTAL_LONG: &str = include_str!("kernels/fractal_long.splash");
    /// Iterations one pass of [`FRACTAL_LONG`] runs.
    pub const LONG_PASS_ITER: u32 = 2048;
    /// The most iterations a node renders ([`FRACTAL_LONG`] at its most passes).
    pub const MAX_ITER: u32 = LONG_PASS_ITER * MAX_PASSES as u32;
    /// Newton's method on z^power - 1: top byte 1 + the root (255 none).
    pub const NEWTON: &str = include_str!("kernels/newton.splash");

    /// What a fractal tile depends on besides its place.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct FractalParams {
        /// 0 Mandelbrot, 1 Julia, 2 Burning Ship, 3 Tricorn, 4 Multibrot,
        /// 5 Newton.
        pub kind: u32,
        /// Multibrot's power (2..8).
        pub power: u32,
        pub max_iter: u32,
        pub julia: (f64, f64),
        /// Distance estimation: escaped texels carry a distance code.
        pub de: bool,
        /// Cross orbit trap in the low 24 bits instead of the smooth count.
        pub trap: bool,
    }

    impl Default for FractalParams {
        fn default() -> Self {
            FractalParams { kind: 0, power: 3, max_iter: 500, julia: (-0.7269, 0.1889), de: false, trap: false }
        }
    }

    /// The tile with top-left corner (x, y) and edge `size`, at `res` texels.
    pub fn fractal_tile(id: u64, x: f64, y: f64, size: f64, res: u32, p: &FractalParams) -> KernelTileJson {
        KernelTileJson {
            id,
            count: res * res,
            params: vec![KernelParamJson { name: "res".into(), value: res as f32 }],
            inputs: vec![KernelBufferJson::from_f64("view", &[x, y, size / res as f64, p.julia.0, p.julia.1])],
        }
    }

    pub fn fractal_job(p: &FractalParams, tiles: Vec<KernelTileJson>) -> KernelJobJson {
        let param = |name: &str, value: u32| KernelParamJson { name: name.into(), value: value as f32 };
        let job = |source: &str, params| KernelJobJson { source: source.to_string(), params, inputs: Vec::new(), tiles, encoding: "delta-deflate".into(), seed: 0, passes: None, feedback: None };
        let (de, trap) = (param("de", p.de as u32), param("trap", p.trap as u32));
        match p.kind {
            // Newton has neither (its texels are roots)
            5 => job(NEWTON, vec![param("power", p.power), param("max_iter", p.max_iter.min(1024))]),
            4 if p.max_iter <= 6144 => job(MULTIBROT, vec![param("power", p.power), param("max_iter", p.max_iter), de, trap]),
            0..=3 if p.max_iter <= 16384 => job(FRACTAL, vec![param("kind", p.kind), param("max_iter", p.max_iter), de, trap]),
            _ => {
                let max_iter = p.max_iter.min(MAX_ITER);
                let passes = max_iter.div_ceil(LONG_PASS_ITER);
                let mut j = job(FRACTAL_LONG, vec![param("kind", p.kind), param("power", p.power), param("max_iter", max_iter), de, trap, param("passes", passes)]);
                j.passes = Some(passes);
                j.feedback = Some(vec![
                    KernelFeedbackJson { output: "sfo".into(), input: "sfi".into() },
                    KernelFeedbackJson { output: "sio".into(), input: "sii".into() },
                ]);
                j
            }
        }
    }
}

fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.b.len()).ok_or_else(|| "truncated tile batch".to_string())?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u32(&mut self) -> Result<u32, String> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn u64(&mut self) -> Result<u64, String> {
        let s = self.take(8)?;
        let mut b = [0u8; 8];
        b.copy_from_slice(s);
        Ok(u64::from_le_bytes(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_round_trips_in_both_encodings() {
        let words: Vec<u32> = (0..5000u32).map(|i| (i / 7) * 64 + if i % 300 == 0 { 255 << 24 } else { 0 }).collect();
        for enc in [ENCODING_RAW, ENCODING_DELTA_DEFLATE] {
            let tile = TileResult {
                id: 42,
                status: STATUS_OK,
                elements: 5000,
                work: 123456,
                nanos: 789,
                reduced: vec![1.5, -2.0],
                outputs: vec![TileOutput { name: "out".into(), words: words.clone() }],
                message: String::new(),
            };
            let mut rec = Vec::new();
            encode_tile(&tile, enc, &mut rec);
            let bytes = batch_bytes(true, &[rec]);
            let batch = decode_batch(&bytes, 5000).unwrap();
            assert!(batch.done);
            assert_eq!(batch.tiles, vec![tile]);
            assert!(decode_batch(&bytes, 4999).is_err(), "an output past the caller's bound is refused");
            assert!(decode_batch(&bytes[..bytes.len() - 1], 5000).is_err());
        }
    }

    #[test]
    fn f64_inputs_are_low_then_high_words() {
        let b = KernelBufferJson::from_f64("view", &[1.5, -0.25]);
        let w = b.decode(16).unwrap();
        assert_eq!(w.len(), 4);
        assert_eq!(((w[1] as u64) << 32 | w[0] as u64), 1.5f64.to_bits());
        assert!(b.decode(3).is_err());
    }
}
