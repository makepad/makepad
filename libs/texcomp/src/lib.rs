//! Texture compression: the block codecs and the container every texture
//! travels in.
//!
//! No dependencies and no `unsafe`. A texture arrives from outside the
//! process — a converted model, a published asset, a file someone dropped in
//! — so every length and offset in here is untrusted input and is checked
//! before it is used.

pub mod bc1;
pub mod ktx2;
pub mod uastc;
pub mod basis;
pub mod astc;
pub mod bc7;
pub mod bc7_tables;
pub mod etc;
pub mod rgtc;
pub mod transcode;
pub mod uastc_tables;

/// Run every chunk on the calling thread. Image encoders accept an executor
/// with this signature: it must call `work` exactly once for every index in
/// `0..chunks`, in any order, and return only after all calls have finished.
pub fn run_inline(chunks: usize, work: &(dyn Fn(usize) + Sync)) {
    for index in 0..chunks {
        work(index);
    }
}

/// Keep whole block-row bands and BC7's existing 512-block hand-off grain,
/// independent of the executor's worker count. Each block occupies 16 bytes.
fn encode_row_chunks(
    out: &mut [u8],
    blocks_x: usize,
    run: &dyn Fn(usize, &(dyn Fn(usize) + Sync)),
    encode: impl Fn(usize, &mut [u8]) + Sync,
) {
    let rows_per = 512usize.div_ceil(blocks_x);
    // One metadata allocation; no chunk buffers or copies. The locks only
    // transfer disjoint exclusive borrows, never surround compression work.
    let slots: Vec<_> = out.chunks_mut(rows_per * blocks_x * 16)
        .map(|rows| std::sync::Mutex::new(Some(rows))).collect();
    run(slots.len(), &|index| {
        let rows = slots[index].lock().unwrap().take().expect("chunk run more than once");
        encode(index * rows_per, rows);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_chunk_order_preserves_encoded_bytes() {
        // Two bands, a short final band, and partial tiles on both edges.
        // Distinct solid tiles keep this small while exposing misplaced bands.
        let (w, h) = (7u32, 1025u32);
        let rgba: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| {
            let tile = (y / 4) * 2 + x / 4;
            [tile as u8, (tile >> 8) as u8, (tile * 47) as u8, 255]
        })).collect();
        let reverse = |chunks: usize, work: &(dyn Fn(usize) + Sync)| {
            assert!(chunks > 1, "exercise chunk ordering");
            for index in (0..chunks).rev() { work(index); }
        };
        assert_eq!(rgtc::encode_bc5_image(&rgba, w, h, &reverse),
            rgtc::encode_bc5_image(&rgba, w, h, &run_inline), "BC5");
        assert_eq!(uastc::encode_image(&rgba, w, h, uastc::Quality::Fast, &reverse).unwrap(),
            uastc::encode_image(&rgba, w, h, uastc::Quality::Fast, &run_inline).unwrap(), "UASTC");
        let bc7 = bc7::encode_image(&rgba, w, h, &run_inline).unwrap();
        assert_eq!(bc7::encode_image(&rgba, w, h, &reverse).unwrap(), bc7, "BC7 RGBA");
        let bgra: Vec<u32> = rgba.chunks_exact(4).map(|p|
            ((p[3] as u32) << 24) | ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32
        ).collect();
        assert_eq!(bc7::encode_bgra_u32_image(&bgra, w, h, &reverse).unwrap(), bc7, "BC7 BGRA");
    }
}
