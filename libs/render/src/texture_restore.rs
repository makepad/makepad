//! Textures the renderer uploads as blocks from a stored form drop their CPU chain once the
//! platform has uploaded it ([`Texture::set_discard_cpu_after_upload`]): the prepared-texture
//! store already holds that chain, so a copy in `Cx` only costs memory (on unified memory as
//! much again as the GPU copy). After a GPU device is replaced the platform uploads every other
//! texture again from its CPU chain; these the renderer remakes from the store instead. On its
//! first draw on a new device generation, and after models are installed, a renderer remakes
//! each such texture its models hold that is not on the current device: on the pool, from the
//! level it held, written back into the same texture.
//!
//! What is known here is per texture, process-wide: a texture uploaded by one renderer may be
//! drawn by the models of another (a model installed in a second pane), or installed again
//! after a replacement it missed.

#[cfg(not(test))]
use crate::material_surface::remake_stored;
use crate::material_surface::{PreparedTexture, STORE_ENCODES};
use makepad_draw::makepad_platform::thread::{Class, TaskHandle};
use makepad_draw::log;
use makepad_draw::makepad_platform::{Cx, Texture, TextureId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Instant;

/// Whether a texture of this form drops its CPU chain after upload: blocks with a stored form
/// to remake them from. Never on the web, whose context restore uploads from CPU chains only.
pub fn drops_cpu_chain(compressed: bool, stored: &[u64]) -> bool {
    !cfg!(target_arch = "wasm32") && compressed && !stored.is_empty()
}

/// What remakes a texture that dropped its CPU chain: its stored-form keys (one, or one per
/// array layer), the first mip it holds, its content key, and the device generation its chain
/// was written for.
struct Held {
    stored: Vec<u64>,
    level: usize,
    hash: u64,
    generation: u64,
}

struct Running {
    texture: Texture,
    level: usize,
    task: TaskHandle<Remade>,
}

#[derive(Default)]
struct Restores {
    held: HashMap<TextureId, Held>,
    running: HashMap<TextureId, Running>,
    /// Textures whose stored form was not in the store yet (its encode is owed), with the
    /// stored-form encode count then: asked again once another encode lands.
    missing: HashMap<TextureId, (Texture, usize)>,
    /// The restore after the last device replacement: when it began, textures and bytes remade.
    round: Option<(Instant, usize, usize)>,
}

thread_local! {
    static RESTORES: RefCell<Restores> = RefCell::default();
}

type Remade = Option<Result<PreparedTexture, String>>;

#[cfg(not(test))]
fn remake(stored: &[u64], level: usize, hash: u64) -> Remade {
    remake_stored(stored, level, hash)
}
#[cfg(test)]
use tests::remake;

fn device_generation(cx: &Cx) -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    return cx.gpu_device_generation();
    #[cfg(target_arch = "wasm32")]
    { let _ = cx; 0 }
}

/// `texture` now holds levels `level..` of the chain stored under `stored` (content key `hash`),
/// uploaded or retuned in this frame: it drops its CPU chain after the upload when its form
/// can be remade, and keeps it otherwise.
pub(crate) fn note(cx: &mut Cx, texture: &Texture, compressed: bool, stored: &[u64], level: usize, hash: u64) {
    let drops = drops_cpu_chain(compressed, stored);
    texture.set_discard_cpu_after_upload(cx, drops);
    let generation = device_generation(cx);
    RESTORES.with(|restores| {
        let mut restores = restores.borrow_mut();
        if drops {
            restores.held.insert(texture.texture_id(), Held { stored: stored.to_vec(), level, hash, generation });
        } else {
            restores.held.remove(&texture.texture_id());
        }
    });
}

/// `id` left a renderer's cache and nothing else holds it.
pub(crate) fn forget(id: TextureId) {
    RESTORES.with(|restores| {
        let mut restores = restores.borrow_mut();
        restores.held.remove(&id);
        restores.running.remove(&id);
        restores.missing.remove(&id);
    });
}

/// Whether any texture dropped its CPU chain (else no renderer needs to look).
pub(crate) fn any_held() -> bool {
    RESTORES.with(|restores| !restores.borrow().held.is_empty())
}

/// Remake each of `textures` that dropped its CPU chain on an earlier device than the current.
pub(crate) fn restore_stale<'a>(cx: &mut Cx, textures: impl Iterator<Item = &'a Texture>) {
    let generation = device_generation(cx);
    RESTORES.with(|restores| {
        let mut restores = restores.borrow_mut();
        for texture in textures {
            let id = texture.texture_id();
            if restores.held.get(&id).is_some_and(|held| held.generation != generation) && !restores.running.contains_key(&id) {
                restores.round.get_or_insert_with(|| (Instant::now(), 0, 0));
                restores.start(cx, texture.clone());
            }
        }
    });
}

/// Once per draw: land finished remakes, and ask again for stored forms an encode has put in
/// the store since. True while remakes run (the caller keeps frames coming to land them).
pub(crate) fn poll(cx: &mut Cx) -> bool {
    RESTORES.with(|restores| restores.borrow_mut().poll(cx))
}

impl Restores {
    fn start(&mut self, cx: &mut Cx, texture: Texture) {
        let id = texture.texture_id();
        let Some(held) = self.held.get(&id) else { return };
        if self.running.contains_key(&id) { return; }
        self.missing.remove(&id);
        let (stored, level, hash) = (held.stored.clone(), held.level, held.hash);
        match cx.task_pool().submit(Class::Visible, move || remake(&stored, level, hash)) {
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
        if self.running.is_empty() { return false; }
        let finished: Vec<TextureId> = self.running.iter().filter(|(_, run)| run.task.is_finished()).map(|(id, _)| *id).collect();
        for id in finished {
            let Some(mut run) = self.running.remove(&id) else { continue };
            let Some(result) = run.task.try_take() else { self.running.insert(id, run); continue };
            let hash = self.held.get(&id).map_or(0, |held| held.hash);
            match result {
                // A retune since (a raise) wrote a newer chain, or the texture was forgotten.
                Ok(Some(Ok(prepared))) => if let Some(held) = self.held.get_mut(&id).filter(|held| held.level == run.level) {
                    held.generation = device_generation(cx);
                    if let Some(round) = self.round.as_mut() { round.1 += 1; round.2 += prepared.bytes(); }
                    prepared.retune(cx, &run.texture);
                },
                Ok(None) => { self.missing.insert(id, (run.texture, encodes)); }
                Ok(Some(Err(error))) => log!("texture restore of {hash:016x}: its stored form is unusable ({error}); it stays empty"),
                Err(error) => log!("texture restore of {hash:016x} failed: {error:?}"),
            }
        }
        if self.running.is_empty() {
            if let Some((began, textures, bytes)) = self.round.take() {
                let waiting = if self.missing.is_empty() { String::new() } else { format!("; {} wait for their stored forms' encodes", self.missing.len()) };
                log!("texture restore: {textures} textures ({:.1} MiB) remade from their stored forms after the GPU device was replaced, in {:.1} s{waiting}",
                    bytes as f64 / 1048576.0, began.elapsed().as_secs_f64());
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material_surface::GpuCompressedFormat;
    use makepad_draw::makepad_platform::TextureFormat;

    /// A 16x16 BC7 chain (16, 4 and 1 blocks) whose every byte is `byte`, stored under key 1.
    fn chain(byte: u8) -> PreparedTexture {
        PreparedTexture { width: 16, height: 16, max_level: 2, hash: 0x77, stored: Some(1), data: Vec::new().into(),
            compressed: Some((GpuCompressedFormat::Bc7, vec![byte; (16 + 4 + 1) * 16].into())) }
    }

    /// The store: key 1 holds [`chain`]`(0x5a)`; key 2's encode is owed (not stored yet).
    pub(super) fn remake(stored: &[u64], level: usize, _hash: u64) -> Remade {
        (stored == [1]).then(|| Ok(chain(0x5a).from_level(level)))
    }

    /// The device the held chains were written for is gone (a test `Cx` stays on generation 0).
    fn replaced() {
        RESTORES.with(|r| for held in r.borrow_mut().held.values_mut() { held.generation = u64::MAX; });
    }

    fn blocks(cx: &mut Cx, texture: &Texture) -> Option<(usize, Vec<u8>)> {
        match texture.get_format(cx) {
            TextureFormat::VecMipCompressed { width, data: Some(data), .. } => Some((*width, data.to_vec())),
            _ => None,
        }
    }

    #[test]
    fn dropped_cpu_chains_are_remade_from_their_stored_forms() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let held = chain(0x11).from_level(1).upload(&mut cx);
        note(&mut cx, &held, true, &[1], 1, 0x77);
        assert!(held.discards_cpu_after_upload(&cx), "blocks with a stored form drop their CPU chain");
        let mut rgba = chain(0x22);
        rgba.compressed = None;
        rgba.data = vec![0u32; 16 * 16].into();
        let rgba = rgba.upload(&mut cx);
        note(&mut cx, &rgba, false, &[1], 0, 0x78);
        assert!(!rgba.discards_cpu_after_upload(&cx), "an RGBA chain keeps its CPU copy");
        let owed = chain(0x33).upload(&mut cx);
        note(&mut cx, &owed, true, &[2], 0, 0x79);

        assert!(any_held());
        restore_stale(&mut cx, [&held, &rgba, &owed].into_iter());
        assert!(!poll(&mut cx), "what is on the current device is not remade");
        // A device replacement: the platform dropped the CPU chains, the textures come back empty.
        for texture in [&held, &owed] {
            if let TextureFormat::VecMipCompressed { data, .. } = texture.get_format(&mut cx) { *data = None; }
        }
        replaced();
        restore_stale(&mut cx, [&held, &rgba, &owed].into_iter());
        let began = Instant::now();
        while poll(&mut cx) {
            assert!(began.elapsed().as_secs() < 10, "the remakes finish");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(blocks(&mut cx, &held), Some((8, vec![0x5a; (4 + 1) * 16])), "the remake starts at the level the texture held");
        assert_eq!(blocks(&mut cx, &owed), None, "a form whose encode is owed waits for it");
        assert!(RESTORES.with(|r| r.borrow().missing.contains_key(&owed.texture_id())), "and is asked again once an encode lands");
        assert!(matches!(rgba.get_format(&mut cx), TextureFormat::VecMipBGRAu8_32 { data: Some(_), .. }), "what kept its CPU chain is the platform's to upload again");

        // A raise wrote a finer chain while a remake ran: the remake is dropped.
        replaced();
        restore_stale(&mut cx, [&held].into_iter());
        chain(0x44).retune(&mut cx, &held);
        note(&mut cx, &held, true, &[1], 0, 0x77);
        while poll(&mut cx) { std::thread::sleep(std::time::Duration::from_millis(1)); }
        assert_eq!(blocks(&mut cx, &held), Some((16, vec![0x44; (16 + 4 + 1) * 16])), "a remake of an older level does not replace a newer chain");
        forget(owed.texture_id());
        assert!(RESTORES.with(|r| !r.borrow().missing.contains_key(&owed.texture_id())), "a forgotten texture is not asked again");
    }
}
