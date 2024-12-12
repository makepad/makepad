/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

use crate::crc::crc_tables::{CRC32_SLICE1_TABLE, CRC32_SLICE8_TABLE};

mod crc_tables;

/// Calculate crc for a data and an initial crc value
///
#[allow(clippy::identity_op, clippy::zero_prefixed_literal)]
pub fn crc32_slice8(data: &[u8], mut crc: u32) -> u32 {
    // main loop
    for chunk in data.chunks_exact(8) {
        let chunk_loaded = u64::from_le_bytes(chunk.try_into().unwrap());

        let v1 = (chunk_loaded & u64::from(u32::MAX)) as u32;
        let v2 = (chunk_loaded >> 32) as u32;

        crc = CRC32_SLICE8_TABLE[0x700 + (((crc ^ v1) >> 00) & 0xFF) as usize]
            ^ CRC32_SLICE8_TABLE[0x600 + (((crc ^ v1) >> 08) & 0xFF) as usize]
            ^ CRC32_SLICE8_TABLE[0x500 + (((crc ^ v1) >> 16) & 0xFF) as usize]
            ^ CRC32_SLICE8_TABLE[0x400 + (((crc ^ v1) >> 24) & 0xFF) as usize]
            ^ CRC32_SLICE8_TABLE[0x300 + (((v2 >> 00) & 0xFF) as usize)]
            ^ CRC32_SLICE8_TABLE[0x200 + (((v2 >> 08) & 0xFF) as usize)]
            ^ CRC32_SLICE8_TABLE[0x100 + (((v2 >> 16) & 0xFF) as usize)]
            ^ CRC32_SLICE8_TABLE[0x000 + (((v2 >> 24) & 0xFF) as usize)];
    }
    // handle remainder
    for remainder in data.chunks_exact(8).remainder() {
        crc = (crc >> 8) ^ CRC32_SLICE1_TABLE[((crc & 0xFF) ^ u32::from(*remainder)) as usize];
    }

    crc
}

/// A smaller CRC function used by hot loops to handle extra data
pub fn _crc32_slice1(data: &[u8], mut crc: u32) -> u32 {
    for datum in data {
        crc = (crc >> 8) ^ CRC32_SLICE1_TABLE[((crc & 0xFF) ^ u32::from(*datum)) as usize];
    }
    crc
}

pub fn calc_crc(data: &[u8]) -> u32 {
    !crc32_slice8(data, u32::MAX)
}

#[test]
fn test_crc_same() {
    use alloc::vec;

    use nanorand::Rng;

    let mut rng = nanorand::WyRand::new();

    let mut data = vec![0_u8; 1000];
    rng.fill(&mut data);

    let crc_simple = _crc32_slice1(&data, 0);
    let crc_table8 = crc32_slice8(&data, 0);

    assert_eq!(
        crc_simple, crc_table8,
        "CRC {crc_simple} {crc_table8} do not match"
    );
}
