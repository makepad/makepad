use super::buffer::{update_hash, LZ_HASH_SHIFT, LZ_HASH_SIZE};
use super::core::{
    flush_block, CallbackOxide, CompressorOxide, TDEFLFlush, TDEFLStatus, LZ_DICT_SIZE,
    LZ_DICT_SIZE_MASK, MAX_MATCH_LEN, MIN_MATCH_LEN,
};
use ::core::cmp;

pub fn compress_stored(d: &mut CompressorOxide, callback: &mut CallbackOxide) -> bool {
    let in_buf = match callback.buf() {
        None => return true,
        Some(in_buf) => in_buf,
    };

    // Make sure this is cleared in case compression level is switched later.
    // TODO: It's possible we don't need this or could do this elsewhere later
    // but just do this here to avoid causing issues for now.
    d.params.saved_match_len = 0;
    let mut bytes_written = d.lz.total_bytes;
    let mut src_pos = d.params.src_pos;
    let mut lookahead_size = d.dict.lookahead_size;
    let mut lookahead_pos = d.dict.lookahead_pos;

    while src_pos < in_buf.len() || (d.params.flush != TDEFLFlush::None && lookahead_size != 0) {
        let src_buf_left = in_buf.len() - src_pos;
        let num_bytes_to_process = cmp::min(src_buf_left, MAX_MATCH_LEN - lookahead_size);

        if lookahead_size + d.dict.size >= usize::from(MIN_MATCH_LEN) - 1
            && num_bytes_to_process > 0
        {
            let dictb = &mut d.dict.b;

            let mut dst_pos = (lookahead_pos + lookahead_size) & LZ_DICT_SIZE_MASK;
            let mut ins_pos = lookahead_pos + lookahead_size - 2;
            // Start the hash value from the first two bytes
            let mut hash = update_hash(
                u16::from(dictb.dict[ins_pos & LZ_DICT_SIZE_MASK]),
                dictb.dict[(ins_pos + 1) & LZ_DICT_SIZE_MASK],
            );

            lookahead_size += num_bytes_to_process;

            for &c in &in_buf[src_pos..src_pos + num_bytes_to_process] {
                // Add byte to input buffer.
                dictb.dict[dst_pos] = c;
                if dst_pos < MAX_MATCH_LEN - 1 {
                    dictb.dict[LZ_DICT_SIZE + dst_pos] = c;
                }

                // Generate hash from the current byte,
                hash = update_hash(hash, c);
                dictb.next[ins_pos & LZ_DICT_SIZE_MASK] = dictb.hash[hash as usize];
                // and insert it into the hash chain.
                dictb.hash[hash as usize] = ins_pos as u16;
                dst_pos = (dst_pos + 1) & LZ_DICT_SIZE_MASK;
                ins_pos += 1;
            }
            src_pos += num_bytes_to_process;
        } else {
            let dictb = &mut d.dict.b;
            for &c in &in_buf[src_pos..src_pos + num_bytes_to_process] {
                let dst_pos = (lookahead_pos + lookahead_size) & LZ_DICT_SIZE_MASK;
                dictb.dict[dst_pos] = c;
                if dst_pos < MAX_MATCH_LEN - 1 {
                    dictb.dict[LZ_DICT_SIZE + dst_pos] = c;
                }

                lookahead_size += 1;
                if lookahead_size + d.dict.size >= MIN_MATCH_LEN.into() {
                    let ins_pos = lookahead_pos + lookahead_size - 3;
                    let hash = ((u32::from(dictb.dict[ins_pos & LZ_DICT_SIZE_MASK])
                        << (LZ_HASH_SHIFT * 2))
                        ^ ((u32::from(dictb.dict[(ins_pos + 1) & LZ_DICT_SIZE_MASK])
                            << LZ_HASH_SHIFT)
                            ^ u32::from(c)))
                        & (LZ_HASH_SIZE as u32 - 1);

                    dictb.next[ins_pos & LZ_DICT_SIZE_MASK] = dictb.hash[hash as usize];
                    dictb.hash[hash as usize] = ins_pos as u16;
                }
            }

            src_pos += num_bytes_to_process;
        }

        d.dict.size = cmp::min(LZ_DICT_SIZE - lookahead_size, d.dict.size);
        if d.params.flush == TDEFLFlush::None && lookahead_size < MAX_MATCH_LEN {
            break;
        }

        let len_to_move = 1;

        bytes_written += 1;

        lookahead_pos += len_to_move;
        assert!(lookahead_size >= len_to_move);
        lookahead_size -= len_to_move;
        d.dict.size = cmp::min(d.dict.size + len_to_move, LZ_DICT_SIZE);

        if bytes_written > 31 * 1024 {
            d.lz.total_bytes = bytes_written;

            d.params.src_pos = src_pos;
            // These values are used in flush_block, so we need to write them back here.
            d.dict.lookahead_size = lookahead_size;
            d.dict.lookahead_pos = lookahead_pos;

            let n = flush_block(d, callback, TDEFLFlush::None)
                .unwrap_or(TDEFLStatus::PutBufFailed as i32);
            if n != 0 {
                return n > 0;
            }
            bytes_written = d.lz.total_bytes;
        }
    }

    d.lz.total_bytes = bytes_written;
    d.params.src_pos = src_pos;
    d.dict.lookahead_size = lookahead_size;
    d.dict.lookahead_pos = lookahead_pos;
    true
}
