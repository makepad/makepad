/*
 * Copyright (c) 2023.
 *
 * This software is free software;
 *
 * You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */
#![allow(dead_code)]

/// An optimized hash chains match finder
///
///
/// # Algorithm
///
/// We keep tabs of two  arrays , `hc_tab` and `next_tab`.
///
/// `hc_tab` contains the data to the first offset chain we think is a potential match
/// and `next_tab` contains other potential match positions/match offsets
///
/// At each point during byte processing, the hash code of the first
/// n bytes(n is influenced by min length) is calculated, this value is
/// then looked up in the hash tab (`hc_tab`) to see if that bucket had a previous
/// offset, if it did, that value is stored into `next_tab` and the value in `hc_tab` is overwritten
///  by the new match position.
///
/// In pseudo code looks like
/// ```text
/// hash_value = calc_hash(bytes);
/// prev_offset = hc_tab[hash_value];
/// hc_tab[hash] = current_position
/// next_tab[current_position]  = prev_offset;
/// ```
///
/// This has an inner beauty to it, i.e to visit previous matches we can easily follow
/// each hash link in what looks like the followinf
///
/// ```text
/// ml = 0;
/// off = 0;
/// while prev_offset != 0
///     new_ml=det_match_length(prev_offset,current_position)
///     if (new_ml > ml){
///         off=prev_offset;
///         ml = new_ml;
///     }
///     prev_offset= next_tab[prev_offset]
/// ````
///
/// This will visit all previous nodes stored for that hash
/// and get the maximum match length from the current position
///
///
/// # Optimizations
///
/// 1. Limit depth search to prevent O(n^2) behaviour
/// 2. Store first match byte in `hc_tab` and `next_tab` entries, this helps us
/// to ensure that we may have a potential match and not a hash collision when checking for a potential match
/// useful because checking for matches incur cache costs
///
/// 3. In case a match is found, and we are still looking for a better match,  check if current match length will go past
///  the previous match length by looking at the byte in current length plus 1
///  if they match, then this has the potential to beat the previous ML, better than reading
///  recalculating the new length if it will just be shorter.
///                    
use alloc::boxed::Box;
use alloc::vec;

use crate::constants::{
    DEFLATE_MAX_BLOCK_SIZE, DEFLATE_MAX_MATCH_LEN, DEFLATE_MIN_LENGTH, DEFLATE_WINDOW_SIZE
};
use crate::encoder::{v_hash, EncodedSequences, MatchSequence};

const HASH_FOUR_LOG_SIZE: usize = 17;
const FIRST_BYTE_OFFSET: u32 = 24;

const HASH_FOUR_SIZE: usize = 1 << HASH_FOUR_LOG_SIZE;

#[inline(never)]
#[allow(clippy::too_many_lines, unused_assignments)]
pub fn compress_block(
    src: &[u8], _dest: &mut [u8], table: &mut HcMatchFinder, sequences: &mut EncodedSequences
) {
    let mut window_start = 0;
    let mut literals_before_match = 0;
    let skip_literals = 1;
    let mut compressed_bytes = 0;

    let mut sequence = MatchSequence::default();

    'match_loop: loop {
        // main match finder loop
        'inner_loop: loop {
            if window_start + skip_literals + DEFLATE_WINDOW_SIZE > src.len() {
                // close to input end
                break 'match_loop;
            }

            if table.longest_four_match(src, window_start, literals_before_match, &mut sequence) {
                sequence.ll = literals_before_match;
                break 'inner_loop;
            }

            window_start += skip_literals;
            literals_before_match += skip_literals;
        }
        compressed_bytes += sequence.ll + sequence.ml;

        sequences.add(sequence);
        table.advance_four_match(src, window_start, sequence.ml);
        literals_before_match = 0;

        window_start += sequence.ml;

        sequence.ml = 0;

        if window_start + DEFLATE_WINDOW_SIZE + skip_literals > src.len() {
            // close to input end
            break 'match_loop;
        }
    }
    {
        assert_eq!(sequence.ml, 0);

        sequence.ll = src
            .len()
            .wrapping_sub(window_start)
            .wrapping_add(literals_before_match);

        sequence.ol = 10;
        sequence.start = src.len() - sequence.ll;
        sequence.ml = DEFLATE_MIN_LENGTH;
        sequences.add(sequence);

        compressed_bytes += sequence.ll;
    }
    table.reset();
    assert_eq!(compressed_bytes, src.len());
}

pub struct HcMatchFinder {
    next_hash:    [usize; 2],
    hc_tab:       [u32; 1 << HASH_FOUR_LOG_SIZE],
    next_tab:     Box<[u32; DEFLATE_MAX_BLOCK_SIZE]>,
    search_depth: i32,
    min_length:   usize,
    nice_length:  usize
}

impl HcMatchFinder {
    /// create a new match finder
    pub fn new(
        buf_size: usize, search_depth: i32, min_length: usize, nice_length: usize
    ) -> HcMatchFinder {
        let n_tab = vec![0; buf_size].into_boxed_slice();
        //debug_assert!(min_length == 4);
        HcMatchFinder {
            next_hash: [0, 0],
            hc_tab: [0; 1 << HASH_FOUR_LOG_SIZE],
            next_tab: n_tab.try_into().expect("Uh oh, fix values bro :)"),
            search_depth,
            nice_length,
            min_length
        }
    }

    pub fn reset(&mut self) {
        self.hc_tab.fill(0);
        self.next_hash.fill(0);
    }
    #[inline(always)]
    pub fn longest_four_match(
        &mut self, bytes: &[u8], start: usize, literal_length: usize, sequence: &mut MatchSequence
    ) -> bool {
        let curr_start = &bytes[start..];
        // store the current first byte in the hash, we use this to
        // determine if a match is either a true mach or a hash collision
        // in the bottom
        let curr_match_byte = usize::from(curr_start[0]);
        let curr_byte = u32::from(curr_start[0]) << FIRST_BYTE_OFFSET;

        let next_window = &bytes[start + 1..];
        /* Get the precomputed hash codes */
        let hash = self.next_hash[1];
        /* From the hash buckets, get the first node of each linked list. */
        let mut cur_offset = self.hc_tab[hash % HASH_FOUR_SIZE] as usize;

        self.hc_tab[hash % HASH_FOUR_SIZE] = curr_byte | (start as u32);
        self.next_tab[start % DEFLATE_MAX_BLOCK_SIZE] = cur_offset as u32;

        //  compute the next hash codes
        let n_hash4 = v_hash(next_window, HASH_FOUR_LOG_SIZE, self.min_length);

        self.next_hash[1] = n_hash4;
        let mut match_found = false;

        if cur_offset != 0 {
            // top byte is usually first match offset, so remove it
            let mut first_match_byte = cur_offset >> FIRST_BYTE_OFFSET;

            cur_offset &= (1 << FIRST_BYTE_OFFSET) - 1;

            let mut depth = self.search_depth;

            'outer: loop {
                if cur_offset == 0 || depth <= 0 {
                    return match_found;
                }
                'inner: loop {
                    depth -= 1;

                    // compare first byte usually stored in hc_tab and next tab for
                    // the offset
                    if first_match_byte == curr_match_byte {
                        // found a possible match, break to see how
                        // long it is
                        // this calls into extend
                        break 'inner;
                    }

                    cur_offset = self.next_tab[cur_offset % DEFLATE_MAX_BLOCK_SIZE] as usize;
                    first_match_byte = cur_offset >> FIRST_BYTE_OFFSET;
                    cur_offset &= (1 << FIRST_BYTE_OFFSET) - 1;

                    if depth <= 0 || cur_offset == 0 {
                        // no match found
                        // go and try the other tab
                        break 'outer;
                    }
                }
                if match_found {
                    // we have a previous match, check if current match length will go past
                    // the previous match length by looking at the byte in current length plus 1
                    // if they match, then this has the potential to beat the previous ML
                    let prev_match_end = bytes[cur_offset + sequence.ml];
                    // N.B: This may read +1 byte past curr_start, but that is okay
                    let curr_match_end = curr_start[sequence.ml];

                    if prev_match_end != curr_match_end {
                        // go to next node
                        cur_offset = self.next_tab[cur_offset % DEFLATE_MAX_BLOCK_SIZE] as usize;
                        first_match_byte = cur_offset >> FIRST_BYTE_OFFSET;
                        cur_offset &= (1 << FIRST_BYTE_OFFSET) - 1;

                        depth -= 1;
                        continue;
                    }
                }
                // extend
                let new_match_length =
                    count(&bytes[cur_offset..], curr_start, DEFLATE_MAX_MATCH_LEN);

                let diff = start - cur_offset;

                if new_match_length >= self.min_length && new_match_length > sequence.ml && diff > 3
                {
                    sequence.ml = new_match_length;
                    sequence.ol = diff;
                    sequence.start = start - literal_length;

                    match_found = true;

                    if new_match_length > self.nice_length {
                        return true;
                    }
                }
                // go to next node
                cur_offset = self.next_tab[cur_offset % DEFLATE_MAX_BLOCK_SIZE] as usize;
                first_match_byte = cur_offset >> FIRST_BYTE_OFFSET;
                cur_offset &= (1 << FIRST_BYTE_OFFSET) - 1;

                depth -= 1;
            }
        }
        match_found
    }

    #[inline(always)]
    pub fn advance_four_match(&mut self, window_start: &[u8], mut start: usize, mut length: usize) {
        if (start + length + 100) < window_start.len() {
            let mut hash4 = self.next_hash[1];
            loop {
                let next_window = &window_start[start + 1..];

                let curr_byte = u32::from(window_start[start]) << FIRST_BYTE_OFFSET;

                self.next_tab[start % DEFLATE_MAX_BLOCK_SIZE] = self.hc_tab[hash4 % HASH_FOUR_SIZE];
                self.hc_tab[hash4 % HASH_FOUR_SIZE] = curr_byte | (start as u32);
                start += 1;
                //  compute the next hash codes
                hash4 = v_hash(next_window, HASH_FOUR_LOG_SIZE, self.min_length);
                length -= 1;

                if length == 0 {
                    break;
                }
            }
            self.next_hash[1] = hash4;
        }
    }
}

pub fn count(window: &[u8], match_window: &[u8], max_match: usize) -> usize {
    /*
     * This is pretty neat and worth an explanation
     * a ^ b ==  0  if a==b
     *
     * If it's not zero the first non-zero bit will indicate that the byte at it's boundary is not the same
     *(e.g if bit 11 is 1 it means byte formed by bits [8..16] are not same). and if their not the same,
     * then our match stops there.
     *
     * Credits to Yann Collet lz4.c for this.
     */

    const SIZE: usize = usize::BITS as usize / 8;

    let mut match_length = 0;

    let window_chunks = window.chunks_exact(SIZE);
    let match_chunks = match_window.chunks_exact(SIZE);
    // number of iterations the loop below can iterate
    // let it be one less max match to ensure we never go past that
    let num_iters = (max_match / SIZE).saturating_sub(1);

    for (sm_window, sm_match) in window_chunks.zip(match_chunks).take(num_iters) {
        let sm_w: usize = usize::from_ne_bytes(sm_window.try_into().unwrap());
        let sm_m: usize = usize::from_ne_bytes(sm_match.try_into().unwrap());
        let diff = sm_w ^ sm_m; // it's associative.

        if diff == 0 {
            match_length += SIZE;
        } else {
            // naa they don't match fully
            match_length += (diff.trailing_zeros() >> 3) as usize;
            return match_length;
        }
    }

    // PS: There is a bug with this, investigate
    //
    // // small chunks
    // match_window[match_length..]
    //     .iter()
    //     .zip(&window[match_length..])
    //     .for_each(|(a, b)| {
    //         if a == b
    //         {
    //             match_length += 1;
    //         }
    //     });

    match_length
}
