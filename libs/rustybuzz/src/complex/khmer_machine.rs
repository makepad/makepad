use crate::buffer::Buffer;

const MACHINE_TRANS_KEYS: &[u8] = &[
    5, 26, 5, 21, 5, 26, 5, 21, 1, 16, 5, 21, 5, 26, 5, 21,
    5, 26, 5, 21, 5, 21, 5, 26, 5, 21, 1, 16, 5, 21, 5, 26,
    5, 21, 5, 26, 5, 21, 5, 26, 1, 29, 5, 29, 5, 29, 5, 29,
    22, 22, 5, 22, 5, 29, 5, 29, 5, 29, 1, 16, 5, 26, 5, 29,
    5, 29, 22, 22, 5, 22, 5, 29, 5, 29, 1, 16, 5, 29, 5, 29,
    0
];

const MACHINE_KEY_SPANS: &[u8] = &[
    22, 17, 22, 17, 16, 17, 22, 17,
    22, 17, 17, 22, 17, 16, 17, 22,
    17, 22, 17, 22, 29, 25, 25, 25,
    1, 18, 25, 25, 25, 16, 22, 25,
    25, 1, 18, 25, 25, 16, 25, 25
];

const MACHINE_INDEX_OFFSETS: &[u16] = &[
    0, 23, 41, 64, 82, 99, 117, 140,
    158, 181, 199, 217, 240, 258, 275, 293,
    316, 334, 357, 375, 398, 428, 454, 480,
    506, 508, 527, 553, 579, 605, 622, 645,
    671, 697, 699, 718, 744, 770, 787, 813
];

const MACHINE_INDICIES: &[u8] = &[
    1, 1, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 2,
    3, 0, 0, 0, 0, 4, 0, 1,
    1, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 3,
    0, 1, 1, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 3, 0, 0, 0, 0, 4, 0,
    5, 5, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    4, 0, 6, 6, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 6, 0, 7, 7, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 8, 0, 9, 9, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 10, 0, 0,
    0, 0, 4, 0, 9, 9, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 10, 0, 11, 11,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 12, 0,
    0, 0, 0, 4, 0, 11, 11, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 12, 0, 14,
    14, 13, 13, 13, 13, 13, 13, 13,
    13, 13, 13, 13, 13, 13, 13, 15,
    13, 14, 14, 16, 16, 16, 16, 16,
    16, 16, 16, 16, 16, 16, 16, 16,
    16, 15, 16, 16, 16, 16, 17, 16,
    18, 18, 16, 16, 16, 16, 16, 16,
    16, 16, 16, 16, 16, 16, 16, 16,
    17, 16, 19, 19, 16, 16, 16, 16,
    16, 16, 16, 16, 16, 16, 16, 16,
    16, 19, 16, 20, 20, 16, 16, 16,
    16, 16, 16, 16, 16, 16, 16, 16,
    16, 16, 16, 21, 16, 22, 22, 16,
    16, 16, 16, 16, 16, 16, 16, 16,
    16, 16, 16, 16, 16, 23, 16, 16,
    16, 16, 17, 16, 22, 22, 16, 16,
    16, 16, 16, 16, 16, 16, 16, 16,
    16, 16, 16, 16, 23, 16, 24, 24,
    16, 16, 16, 16, 16, 16, 16, 16,
    16, 16, 16, 16, 16, 16, 25, 16,
    16, 16, 16, 17, 16, 24, 24, 16,
    16, 16, 16, 16, 16, 16, 16, 16,
    16, 16, 16, 16, 16, 25, 16, 14,
    14, 16, 16, 16, 16, 16, 16, 16,
    16, 16, 16, 16, 16, 16, 26, 15,
    16, 16, 16, 16, 17, 16, 28, 28,
    27, 27, 29, 29, 27, 27, 27, 27,
    2, 2, 27, 30, 27, 28, 27, 27,
    27, 27, 15, 19, 27, 27, 27, 17,
    23, 25, 21, 27, 32, 32, 31, 31,
    31, 31, 31, 31, 31, 33, 31, 31,
    31, 31, 31, 2, 3, 6, 31, 31,
    31, 4, 10, 12, 8, 31, 34, 34,
    31, 31, 31, 31, 31, 31, 31, 35,
    31, 31, 31, 31, 31, 31, 3, 6,
    31, 31, 31, 4, 10, 12, 8, 31,
    5, 5, 31, 31, 31, 31, 31, 31,
    31, 35, 31, 31, 31, 31, 31, 31,
    4, 6, 31, 31, 31, 31, 31, 31,
    8, 31, 6, 31, 7, 7, 31, 31,
    31, 31, 31, 31, 31, 35, 31, 31,
    31, 31, 31, 31, 8, 6, 31, 36,
    36, 31, 31, 31, 31, 31, 31, 31,
    35, 31, 31, 31, 31, 31, 31, 10,
    6, 31, 31, 31, 4, 31, 31, 8,
    31, 37, 37, 31, 31, 31, 31, 31,
    31, 31, 35, 31, 31, 31, 31, 31,
    31, 12, 6, 31, 31, 31, 4, 10,
    31, 8, 31, 34, 34, 31, 31, 31,
    31, 31, 31, 31, 33, 31, 31, 31,
    31, 31, 31, 3, 6, 31, 31, 31,
    4, 10, 12, 8, 31, 28, 28, 31,
    31, 31, 31, 31, 31, 31, 31, 31,
    31, 31, 31, 31, 28, 31, 14, 14,
    38, 38, 38, 38, 38, 38, 38, 38,
    38, 38, 38, 38, 38, 38, 15, 38,
    38, 38, 38, 17, 38, 40, 40, 39,
    39, 39, 39, 39, 39, 39, 41, 39,
    39, 39, 39, 39, 39, 15, 19, 39,
    39, 39, 17, 23, 25, 21, 39, 18,
    18, 39, 39, 39, 39, 39, 39, 39,
    41, 39, 39, 39, 39, 39, 39, 17,
    19, 39, 39, 39, 39, 39, 39, 21,
    39, 19, 39, 20, 20, 39, 39, 39,
    39, 39, 39, 39, 41, 39, 39, 39,
    39, 39, 39, 21, 19, 39, 42, 42,
    39, 39, 39, 39, 39, 39, 39, 41,
    39, 39, 39, 39, 39, 39, 23, 19,
    39, 39, 39, 17, 39, 39, 21, 39,
    43, 43, 39, 39, 39, 39, 39, 39,
    39, 41, 39, 39, 39, 39, 39, 39,
    25, 19, 39, 39, 39, 17, 23, 39,
    21, 39, 44, 44, 39, 39, 39, 39,
    39, 39, 39, 39, 39, 39, 39, 39,
    39, 44, 39, 45, 45, 39, 39, 39,
    39, 39, 39, 39, 30, 39, 39, 39,
    39, 39, 26, 15, 19, 39, 39, 39,
    17, 23, 25, 21, 39, 40, 40, 39,
    39, 39, 39, 39, 39, 39, 30, 39,
    39, 39, 39, 39, 39, 15, 19, 39,
    39, 39, 17, 23, 25, 21, 39, 0
];

const MACHINE_TRANS_TARGS: &[u8] = &[
    20, 1, 28, 22, 23, 3, 24, 5,
    25, 7, 26, 9, 27, 20, 10, 31,
    20, 32, 12, 33, 14, 34, 16, 35,
    18, 36, 39, 20, 21, 30, 37, 20,
    0, 29, 2, 4, 6, 8, 20, 20,
    11, 13, 15, 17, 38, 19
];

const MACHINE_TRANS_ACTIONS: &[u8] = &[
    1, 0, 2, 2, 2, 0, 0, 0,
    2, 0, 2, 0, 2, 3, 0, 4,
    5, 2, 0, 0, 0, 2, 0, 2,
    0, 2, 4, 8, 2, 9, 0, 10,
    0, 0, 0, 0, 0, 0, 11, 12,
    0, 0, 0, 0, 4, 0
];

const MACHINE_TO_STATE_ACTIONS: &[u8] = &[
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 6, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0
];

const MACHINE_FROM_STATE_ACTIONS: &[u8] = &[
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 7, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0
];

const MACHINE_EOF_TRANS: &[u8] = &[
    1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 14, 17, 17, 17, 17, 17,
    17, 17, 17, 17, 0, 32, 32, 32,
    32, 32, 32, 32, 32, 32, 39, 40,
    40, 40, 40, 40, 40, 40, 40, 40
];

#[derive(Clone, Copy)]
pub enum SyllableType {
    ConsonantSyllable = 0,
    BrokenCluster,
    NonKhmerCluster,
}

pub fn find_syllables_khmer(buffer: &mut Buffer) {
    let mut cs = 20usize;
    let mut ts = 0;
    let mut te = 0;
    let mut act = 0;
    let mut p = 0;
    let pe = buffer.len;
    let eof = buffer.len;
    let mut syllable_serial = 1u8;
    let mut reset = true;
    let mut slen;
    let mut trans = 0;
    if p == pe {
        if MACHINE_EOF_TRANS[cs] > 0 {
            trans = (MACHINE_EOF_TRANS[cs] - 1) as usize;
        }
    }

    loop {
        if reset {
            if MACHINE_FROM_STATE_ACTIONS[cs] == 7 {
                ts = p;
            }

            slen = MACHINE_KEY_SPANS[cs] as usize;
            let cs_idx = ((cs as i32) << 1) as usize;
            let i = if slen > 0 &&
                MACHINE_TRANS_KEYS[cs_idx] <= buffer.info[p].indic_category() as u8 &&
                buffer.info[p].indic_category() as u8 <= MACHINE_TRANS_KEYS[cs_idx + 1]
            {
                (buffer.info[p].indic_category() as u8 - MACHINE_TRANS_KEYS[cs_idx]) as usize
            } else {
                slen
            };
            trans = MACHINE_INDICIES[MACHINE_INDEX_OFFSETS[cs] as usize + i] as usize;
        }
        reset = true;

        cs = MACHINE_TRANS_TARGS[trans] as usize;

        if MACHINE_TRANS_ACTIONS[trans] != 0 {
            match MACHINE_TRANS_ACTIONS[trans] {
                2 => te = p + 1,
                8 => {
                    te = p + 1;
                    found_syllable(ts, te, &mut syllable_serial, SyllableType::NonKhmerCluster, buffer);
                }
                10 => {
                    te = p;
                    p -= 1;
                    found_syllable(ts, te, &mut syllable_serial, SyllableType::ConsonantSyllable, buffer);
                }
                12 => {
                    te = p;
                    p -= 1;
                    found_syllable(ts, te, &mut syllable_serial, SyllableType::BrokenCluster, buffer);
                }
                11 => {
                    te = p;
                    p -= 1;
                    found_syllable(ts, te, &mut syllable_serial, SyllableType::NonKhmerCluster, buffer);
                }
                1 => {
                    p = te - 1;
                    found_syllable(ts, te, &mut syllable_serial, SyllableType::ConsonantSyllable, buffer);
                }
                5 => {
                    p = te - 1;
                    found_syllable(ts, te, &mut syllable_serial, SyllableType::BrokenCluster, buffer);
                }
                3 => {
                    match act {
                        2 => {
                            p = te - 1;
                            found_syllable(ts, te, &mut syllable_serial, SyllableType::BrokenCluster, buffer);
                        }
                        3 => {
                            p = te - 1;
                            found_syllable(ts, te, &mut syllable_serial, SyllableType::NonKhmerCluster, buffer);
                        }
                        _ => {}
                    }
                }
                4 => {
                    te = p + 1;
                    act = 2;
                }
                9 => {
                    te = p + 1;
                    act = 3;
                }
                _ => {}
            }
        }

        if MACHINE_TO_STATE_ACTIONS[cs] == 6 {
            ts = 0;
        }

        p += 1;
        if p != pe {
            continue;
        }

        if p == eof {
            if MACHINE_EOF_TRANS[cs] > 0 {
                trans = (MACHINE_EOF_TRANS[cs] - 1) as usize;
                reset = false;
                continue;
            }
        }

        break;
    }
}

#[inline]
fn found_syllable(
    start: usize,
    end: usize,
    syllable_serial: &mut u8,
    kind: SyllableType,
    buffer: &mut Buffer,
) {
    for i in start..end {
        buffer.info[i].set_syllable((*syllable_serial << 4) | kind as u8);
    }

    *syllable_serial += 1;

    if *syllable_serial == 16 {
        *syllable_serial = 1;
    }
}
