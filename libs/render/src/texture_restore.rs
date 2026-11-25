//! Textures the renderer uploads as blocks from a stored form drop their CPU chain once the
//! platform has uploaded it ([`Texture::set_discard_cpu_after_upload`]): the prepared-texture
//! store already holds that chain, so a copy in `Cx` only costs memory (on unified memory as
//! much again as the GPU copy). After a GPU device is replaced the platform uploads every other
//! texture again from its CPU chain; these the renderer remakes from the store instead. On its
//! first draw on a new device generation, and after models are installed, a renderer remakes
//! each such texture its models hold that is not on the current device: on the pool, from the
//! level it held, written back into the same texture. Each keeps its coarsest levels on the
//! CPU (a few KiB), written into it in the frame the replacement is seen: until its remake
//! lands it draws blurred, never empty.
//!
//! What is known here is per texture, process-wide: a texture uploaded by one renderer may be
//! drawn by the models of another (a model installed in a second pane), or installed again
//! after a replacement it missed.

#[cfg(not(test))]
#[cfg(not(target_arch = "wasm32"))]
use crate::material_surface::remake_stored;
#[cfg(not(target_arch = "wasm32"))]
use crate::material_surface::{PreparedTexture, STORE_ENCODES};
#[cfg(not(target_arch = "wasm32"))]
use makepad_draw::makepad_platform::thread::{Class, TaskHandle};
#[cfg(not(target_arch = "wasm32"))]
use makepad_draw::log;
use makepad_draw::makepad_platform::{Cx, Texture, TextureFormat, TextureId};
#[cfg(not(target_arch = "wasm32"))]
use makepad_draw::makepad_platform::{CompressedTextureFormat, TextureUpdated};
#[cfg(not(target_arch = "wasm32"))]
use std::cell::RefCell;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::{HashMap, HashSet};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::Ordering::Relaxed;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

/// Whether a texture of this form drops its CPU chain after upload: blocks with a stored form
/// to remake them from. Never on the web, whose context restore uploads from CPU chains only.
pub fn drops_cpu_chain(compressed: bool, stored: &[u64]) -> bool {
    !cfg!(target_arch = "wasm32") && compressed && !stored.is_empty()
}

/// A texture that drops its CPU chain keeps its levels of at most this many texels on a side
/// (about 22 KB of a BC7 chain, per array layer).
pub(crate) const TAIL_EDGE: usize = 128;

/// What remakes a texture that dropped its CPU chain: its stored-form keys (one, or one per
/// array layer), the first mip it holds, its content key, the device generation its chain
/// was written for, and its GPU bytes.
#[cfg(not(target_arch = "wasm32"))]
struct Held {
    stored: Vec<u64>,
    level: usize,
    hash: u64,
    generation: u64,
    bytes: usize,
    /// Its coarsest levels (at most [`TAIL_EDGE`] texels on a side) as a texture format, and
    /// whether they are its whole chain: written into the texture when a replacement emptied it.
    tail: Option<(TextureFormat, bool)>,
}

#[cfg(not(target_arch = "wasm32"))]
struct Running {
    texture: Texture,
    level: usize,
    task: TaskHandle<Done>,
}

/// A remake's result, how long it waited for a worker and ran, and when it was done.
#[cfg(not(target_arch = "wasm32"))]
struct Done {
    remade: Remade,
    queued: Duration,
    ran: Duration,
    at: Instant,
}

/// The restore after the last device replacement.
#[cfg(not(target_arch = "wasm32"))]
struct Round {
    began: Instant,
    /// Textures remade and their bytes; textures whose kept tail was their whole chain.
    remade: usize,
    bytes: usize,
    whole_tails: usize,
    /// Worker time of the remakes, the longest one, the longest wait for a worker, and when
    /// the last was done.
    work: Duration,
    longest: Duration,
    queued: Duration,
    last_done: Option<Instant>,
    /// Draws that polled while remakes ran.
    draws: usize,
}

#[derive(Default)]
#[cfg(not(target_arch = "wasm32"))]
struct Restores {
    held: HashMap<TextureId, Held>,
    running: HashMap<TextureId, Running>,
    /// Textures whose stored form was not in the store yet (its encode is owed), with the
    /// stored-form encode count then: asked again once another encode lands.
    missing: HashMap<TextureId, (Texture, usize)>,
    round: Option<Round>,
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static RESTORES: RefCell<Restores> = RefCell::default();
}

#[cfg(not(target_arch = "wasm32"))]
type Remade = Option<Result<PreparedTexture, String>>;

#[cfg(not(test))]
#[cfg(not(target_arch = "wasm32"))]
fn remake(stored: &[u64], level: usize, hash: u64) -> Remade {
    remake_stored(stored, level, hash)
}
#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
use tests::remake;

#[cfg(not(target_arch = "wasm32"))]
fn device_generation(cx: &Cx) -> u64 {
    cx.gpu_device_generation()
}

#[cfg(not(target_arch = "wasm32"))]
fn mib(bytes: usize) -> f64 { bytes as f64 / 1048576.0 }

fn first_coarse_level(width: usize, height: usize, max_level: usize) -> Option<usize> {
    (0..=max_level).find(|&level| (width.max(height) >> level) <= TAIL_EDGE)
}

/// Borrow just the finest level that fits the tail. The size function counts slice elements
/// (BGRA words or block bytes). A short chain may have no level that fits.
pub(crate) fn coarse_level<T>(
    mut width: usize, mut height: usize, max_level: Option<usize>, mut data: &[T],
    size: fn(usize, usize) -> usize,
) -> Option<(usize, usize, &[T])> {
    if width == 0 || height == 0 { return None; }
    let first = first_coarse_level(width, height, max_level.unwrap_or(0))?;
    for _ in 0..first {
        data = data.get(size(width, height)..)?;
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }
    Some((width, height, data.get(..size(width, height))?))
}

/// The device restore's kept tail. Cloning its block buffer shares the allocation.
pub(crate) fn held_tail(id: TextureId) -> Option<TextureFormat> {
    #[cfg(not(target_arch = "wasm32"))]
    return RESTORES.with(|restores| {
        let restores = restores.borrow();
        let (tail, _) = restores.held.get(&id)?.tail.as_ref()?;
        Some(tail.clone())
    });
    #[cfg(target_arch = "wasm32")]
    { let _ = id; None }
}

/// The coarsest levels of a block chain (each layer's levels whose larger side is at most
/// [`TAIL_EDGE`]), as a format to write into the texture, with whether they are all of it.
#[cfg(not(target_arch = "wasm32"))]
fn coarse_tail(format: &TextureFormat) -> Option<(TextureFormat, bool)> {
    let (width, height, layers, format, data, max_level, wrap, array) = match format {
        TextureFormat::VecMipCompressed { width, height, format, data: Some(data), max_level, wrap, .. } =>
            (*width, *height, 1, *format, data, max_level.unwrap_or(0), *wrap, false),
        TextureFormat::VecArrayMipCompressed { width, height, layers, format, data: Some(data), max_level, wrap, .. } =>
            (*width, *height, *layers, *format, data, max_level.unwrap_or(0), *wrap, true),
        _ => return None,
    };
    let first = first_coarse_level(width, height, max_level).unwrap_or(max_level);
    let size = |level: usize| CompressedTextureFormat::level_bytes((width >> level).max(1), (height >> level).max(1));
    let stride: usize = (0..=max_level).map(size).sum();
    let skip: usize = (0..first).map(size).sum();
    if data.len() < stride * layers { return None; }
    let data = if first == 0 { data.clone() } else {
        let mut tail = Vec::with_capacity((stride - skip) * layers);
        for layer in 0..layers { tail.extend_from_slice(&data[layer * stride + skip..(layer + 1) * stride]); }
        tail.into()
    };
    let (width, height, max_level) = ((width >> first).max(1), (height >> first).max(1), Some(max_level - first));
    let updated = TextureUpdated::Full;
    let tail = if array {
        TextureFormat::VecArrayMipCompressed { width, height, layers, format, data: Some(data), max_level, wrap, updated }
    } else {
        TextureFormat::VecMipCompressed { width, height, format, data: Some(data), max_level, wrap, updated }
    };
    Some((tail, first == 0))
}

/// Bytes of the CPU chain a block texture holds (0 once uploaded and dropped).
#[cfg(not(target_arch = "wasm32"))]
fn chain_bytes(format: &TextureFormat) -> usize {
    match format {
        TextureFormat::VecMipCompressed { data: Some(data), .. } | TextureFormat::VecArrayMipCompressed { data: Some(data), .. } => data.len(),
        _ => 0,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn tail_bytes(held: &Held) -> usize { held.tail.as_ref().map_or(0, |(tail, _)| chain_bytes(tail)) }

/// `texture` now holds levels `level..` of the chain stored under `stored` (content key `hash`),
/// uploaded or retuned in this frame: it drops its CPU chain after the upload when its form
/// can be remade, keeping its coarsest levels, and keeps the whole chain otherwise.
pub(crate) fn note(cx: &mut Cx, texture: &Texture, compressed: bool, stored: &[u64], level: usize, hash: u64) {
    let drops = drops_cpu_chain(compressed, stored);
    texture.set_discard_cpu_after_upload(cx, drops);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let generation = device_generation(cx);
        let format = texture.get_format(cx);
        let (tail, bytes) = if drops { (coarse_tail(format), chain_bytes(format)) } else { (None, 0) };
        RESTORES.with(|restores| {
            let mut restores = restores.borrow_mut();
            if drops {
                restores.held.insert(texture.texture_id(), Held { stored: stored.to_vec(), level, hash, generation, bytes, tail });
            } else {
                restores.held.remove(&texture.texture_id());
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (level, hash);
}

/// `id` left a renderer's cache and nothing else holds it.
pub(crate) fn forget(id: TextureId) {
    #[cfg(not(target_arch = "wasm32"))]
    RESTORES.with(|restores| {
        let mut restores = restores.borrow_mut();
        restores.held.remove(&id);
        restores.running.remove(&id);
        restores.missing.remove(&id);
    });
    #[cfg(target_arch = "wasm32")]
    let _ = id;
}

/// Whether any texture dropped its CPU chain (else no renderer needs to look).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn any_held() -> bool {
    RESTORES.with(|restores| !restores.borrow().held.is_empty())
}

/// Remake each of `textures` that dropped its CPU chain on an earlier device than the current:
/// its kept tail is written into it now, its remake starts on the pool, the largest first.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn restore_stale<'a>(cx: &mut Cx, textures: impl Iterator<Item = &'a Texture>) {
    let generation = device_generation(cx);
    RESTORES.with(|restores| {
        let mut restores = restores.borrow_mut();
        let mut seen = HashSet::new();
        let mut stale: Vec<(usize, Texture)> = textures.filter_map(|texture| {
            let id = texture.texture_id();
            let held = restores.held.get(&id)?;
            (held.generation != generation && !restores.running.contains_key(&id) && seen.insert(id)).then(|| (held.bytes, texture.clone()))
        }).collect();
        if stale.is_empty() { return; }
        // a round lasts as long as its longest remake: that one starts first
        stale.sort_by_key(|(bytes, _)| std::cmp::Reverse(*bytes));
        let opened = restores.round.is_none();
        if opened {
            restores.round = Some(Round { began: Instant::now(), remade: 0, bytes: 0, whole_tails: 0, work: Duration::ZERO,
                longest: Duration::ZERO, queued: Duration::ZERO, last_done: None, draws: 0 });
        }
        let (mut remaking, mut bytes, mut tails, mut whole, mut chained) = (0, 0, 0, 0, 0);
        for (size, texture) in stale {
            let tail = restores.held.get(&texture.texture_id()).map_or(0, tail_bytes);
            match restores.replaced(cx, texture, generation) {
                Replaced::Remaking => { remaking += 1; bytes += size; tails += tail; }
                Replaced::Whole => whole += 1,
                Replaced::HoldsChain => chained += 1,
            }
        }
        if opened {
            log!("texture restore: the GPU device was replaced; remaking {remaking} textures ({:.1} MiB) from their stored forms, drawn from their kept coarse levels ({} KiB) until then; {whole} small ones are whole in their kept levels, {chained} not uploaded yet still hold their chain",
                mib(bytes), tails / 1024);
        }
    });
}

/// Once per draw: land finished remakes, and ask again for stored forms an encode has put in
/// the store since. True while remakes run (the caller keeps frames coming to land them).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn poll(cx: &mut Cx) -> bool {
    RESTORES.with(|restores| restores.borrow_mut().poll(cx))
}

/// What became of a texture whose device was replaced.
#[cfg(not(target_arch = "wasm32"))]
enum Replaced {
    /// Its kept levels went in and its remake runs.
    Remaking,
    /// Its kept levels were its whole chain.
    Whole,
    /// It was not uploaded and dropped yet: the platform uploads its chain.
    HoldsChain,
}

#[cfg(not(target_arch = "wasm32"))]
impl Restores {
    /// `texture`'s device was replaced since its chain was written: what still holds its CPU chain
    /// the platform uploads again; else its kept tail goes in now, and its remake starts unless
    /// that tail was the whole chain.
    fn replaced(&mut self, cx: &mut Cx, texture: Texture, generation: u64) -> Replaced {
        let id = texture.texture_id();
        let Some(held) = self.held.get_mut(&id) else { return Replaced::HoldsChain };
        if chain_bytes(texture.get_format(cx)) > 0 {
            held.generation = generation;
            return Replaced::HoldsChain;
        }
        if let Some((tail, whole)) = &held.tail {
            *texture.get_format(cx) = tail.clone();
            if *whole {
                held.generation = generation;
                if let Some(round) = self.round.as_mut() { round.whole_tails += 1; }
                return Replaced::Whole;
            }
        }
        self.start(cx, texture);
        Replaced::Remaking
    }

    fn start(&mut self, cx: &mut Cx, texture: Texture) {
        let id = texture.texture_id();
        let Some(held) = self.held.get(&id) else { return };
        if self.running.contains_key(&id) { return; }
        self.missing.remove(&id);
        let (stored, level, hash) = (held.stored.clone(), held.level, held.hash);
        let submitted = Instant::now();
        let work = move || {
            let began = Instant::now();
            let remade = remake(&stored, level, hash);
            Done { remade, queued: began - submitted, ran: began.elapsed(), at: Instant::now() }
        };
        match cx.task_pool().submit(Class::Visible, work) {
            Ok(task) => { self.running.insert(id, Running { texture, level, task }); }
            Err(error) => log!("texture restore of {hash:016x} not submitted: {error:?}"),
        }
    }

    fn poll(&mut self, cx: &mut Cx) -> bool {
        let encodes = STORE_ENCODES[0].load(Relaxed);
        if !self.missing.is_empty() {
            let again: Vec<Texture> = self.missing.values().filter(|(_, seen)| *seen != encodes).map(|(texture, _)| texture.clone()).collect();
            for texture in again { self.start(cx, texture); }
        }
        if self.running.is_empty() {
            self.round_done();
            return false;
        }
        if let Some(round) = self.round.as_mut() { round.draws += 1; }
        let finished: Vec<TextureId> = self.running.iter().filter(|(_, run)| run.task.is_finished()).map(|(id, _)| *id).collect();
        for id in finished {
            let Some(mut run) = self.running.remove(&id) else { continue };
            let Some(result) = run.task.try_take() else { self.running.insert(id, run); continue };
            let hash = self.held.get(&id).map_or(0, |held| held.hash);
            let done = match result {
                Ok(done) => done,
                Err(error) => { log!("texture restore of {hash:016x} failed: {error:?}; it draws its coarse levels"); continue; }
            };
            if let Some(round) = self.round.as_mut() {
                round.work += done.ran;
                round.longest = round.longest.max(done.ran);
                round.queued = round.queued.max(done.queued);
                round.last_done = round.last_done.max(Some(done.at));
            }
            match done.remade {
                // A retune since (a raise) wrote a newer chain, or the texture was forgotten.
                Some(Ok(prepared)) => if let Some(held) = self.held.get_mut(&id).filter(|held| held.level == run.level) {
                    held.generation = device_generation(cx);
                    if let Some(round) = self.round.as_mut() { round.remade += 1; round.bytes += prepared.bytes(); }
                    prepared.retune(cx, &run.texture);
                },
                None => { self.missing.insert(id, (run.texture, encodes)); }
                Some(Err(error)) => log!("texture restore of {hash:016x}: its stored form is unusable ({error}); it draws its coarse levels"),
            }
        }
        if self.running.is_empty() { self.round_done(); }
        true
    }

    fn round_done(&mut self) {
        let Some(round) = self.round.take() else { return };
        let waiting = if self.missing.is_empty() { String::new() } else { format!("; {} wait for their stored forms' encodes", self.missing.len()) };
        let whole = if round.whole_tails == 0 { String::new() } else { format!("; {} small ones were whole in their kept levels", round.whole_tails) };
        let done = round.last_done.map_or(0.0, |at| (at - round.began).as_secs_f64());
        log!("texture restore: {} textures ({:.1} MiB) remade from their stored forms after the GPU device was replaced, in {:.2} s: {:.2} s of worker time (longest {:.2} s, waited up to {:.2} s for a worker), the last done at {done:.2} s, landed over {} draws{whole}{waiting}",
            round.remade, mib(round.bytes), round.began.elapsed().as_secs_f64(), round.work.as_secs_f64(), round.longest.as_secs_f64(),
            round.queued.as_secs_f64(), round.draws);
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;
    use crate::material_surface::GpuCompressedFormat;
    use makepad_draw::makepad_platform::TextureFormat;

    /// Bytes of a BC7 chain from `edge` texels square down to 1.
    fn chain_len(edge: usize) -> usize {
        (0..=edge.trailing_zeros() as usize).map(|level| CompressedTextureFormat::level_bytes(edge >> level, edge >> level)).sum()
    }

    /// A BC7 chain `edge` texels square whose every byte is `byte`, stored under key 1.
    fn chain_of(edge: usize, byte: u8) -> PreparedTexture {
        PreparedTexture { width: edge, height: edge, max_level: edge.trailing_zeros() as usize, hash: 0x77, stored: Some(1),
            data: Vec::new().into(), compressed: Some((GpuCompressedFormat::Bc7, vec![byte; chain_len(edge)].into())) }
    }
    fn chain(byte: u8) -> PreparedTexture { chain_of(512, byte) }

    /// The store: key 1 holds [`chain`]`(0x5a)`; key 2's encode is owed (not stored yet).
    pub(super) fn remake(stored: &[u64], level: usize, _hash: u64) -> Remade {
        (stored == [1]).then(|| Ok(chain(0x5a).from_level(level)))
    }

    /// The device the held chains were written for is gone (a test `Cx` stays on generation 0),
    /// and with it the CPU chains the platform dropped: the textures come back empty.
    fn replaced(cx: &mut Cx, dropped: &[&Texture]) {
        RESTORES.with(|r| for held in r.borrow_mut().held.values_mut() { held.generation = u64::MAX; });
        for texture in dropped {
            if let TextureFormat::VecMipCompressed { data, .. } = texture.get_format(cx) { *data = None; }
        }
    }

    fn blocks(cx: &mut Cx, texture: &Texture) -> Option<(usize, Vec<u8>)> {
        match texture.get_format(cx) {
            TextureFormat::VecMipCompressed { width, data: Some(data), .. } => Some((*width, data.to_vec())),
            _ => None,
        }
    }

    fn settle(cx: &mut Cx) {
        let began = Instant::now();
        while poll(cx) {
            assert!(began.elapsed().as_secs() < 10, "the remakes finish");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn dropped_cpu_chains_are_remade_from_their_stored_forms() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let held = chain(0x11).from_level(1).upload(&mut cx);
        note(&mut cx, &held, true, &[1], 1, 0x77);
        assert!(held.discards_cpu_after_upload(&cx), "blocks with a stored form drop their CPU chain");
        let mut rgba = chain_of(16, 0x22);
        rgba.compressed = None;
        rgba.data = vec![0u32; 16 * 16].into();
        let rgba = rgba.upload(&mut cx);
        note(&mut cx, &rgba, false, &[1], 0, 0x78);
        assert!(!rgba.discards_cpu_after_upload(&cx), "an RGBA chain keeps its CPU copy");
        let owed = chain(0x33).upload(&mut cx);
        note(&mut cx, &owed, true, &[2], 0, 0x79);
        let small = chain_of(64, 0x66).upload(&mut cx);
        note(&mut cx, &small, true, &[1], 0, 0x7a);

        assert!(any_held());
        restore_stale(&mut cx, [&held, &rgba, &owed, &small].into_iter());
        assert!(!poll(&mut cx), "what is on the current device is not remade");
        replaced(&mut cx, &[&held, &owed, &small]);
        restore_stale(&mut cx, [&held, &rgba, &owed, &small].into_iter());
        let coarse = |byte: u8| Some((128, vec![byte; chain_len(128)]));
        assert_eq!(blocks(&mut cx, &held), coarse(0x11), "until its remake lands a texture draws its kept coarse levels, never empty");
        assert_eq!(blocks(&mut cx, &small), Some((64, vec![0x66; chain_len(64)])), "a chain no larger than the kept levels is whole again at once");
        assert!(RESTORES.with(|r| !r.borrow().running.contains_key(&small.texture_id())), "and is not remade");
        settle(&mut cx);
        assert_eq!(blocks(&mut cx, &held), Some((256, vec![0x5a; chain_len(256)])), "the remake starts at the level the texture held");
        assert_eq!(blocks(&mut cx, &owed), coarse(0x33), "a form whose encode is owed draws its coarse levels while it waits");
        assert!(RESTORES.with(|r| r.borrow().missing.contains_key(&owed.texture_id())), "and is asked again once an encode lands");
        assert!(matches!(rgba.get_format(&mut cx), TextureFormat::VecMipBGRAu8_32 { data: Some(_), .. }), "what kept its CPU chain is the platform's to upload again");

        // A replacement seen before the platform dropped a chain: the platform uploads it again.
        let fresh = chain(0x77).upload(&mut cx);
        note(&mut cx, &fresh, true, &[1], 0, 0x7b);
        replaced(&mut cx, &[]);
        restore_stale(&mut cx, [&fresh].into_iter());
        assert!(RESTORES.with(|r| !r.borrow().running.contains_key(&fresh.texture_id())), "a texture still holding its chain is not remade");
        assert_eq!(blocks(&mut cx, &fresh), Some((512, vec![0x77; chain_len(512)])), "nor replaced by its coarse levels");

        // A raise wrote a finer chain while a remake ran: the remake is dropped.
        replaced(&mut cx, &[&held]);
        restore_stale(&mut cx, [&held].into_iter());
        chain(0x44).retune(&mut cx, &held);
        note(&mut cx, &held, true, &[1], 0, 0x77);
        settle(&mut cx);
        assert_eq!(blocks(&mut cx, &held), Some((512, vec![0x44; chain_len(512)])), "a remake of an older level does not replace a newer chain");
        forget(owed.texture_id());
        assert!(RESTORES.with(|r| !r.borrow().missing.contains_key(&owed.texture_id())), "a forgotten texture is not asked again");
    }
}
