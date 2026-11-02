#![cfg(test)]
#![allow(non_upper_case_globals)]
#![allow(dead_code)]

extern crate brotli_decompressor;
extern crate core;

use std::io::Write;

use super::brotli::enc::BrotliEncoderParams;
use super::integration_tests::UnlimitedBuffer;
use super::Rebox;


#[test]
fn test_custom_dict_minimal() {
    let mut raw = UnlimitedBuffer::new("\012345656789abcde".as_bytes());
    let mut params = BrotliEncoderParams::default();
    params.quality = 10;
    let mut br = UnlimitedBuffer::new(&[]);
    let mut rt = UnlimitedBuffer::new(&[]);
    let dict = "123456789abcde".as_bytes();
    super::compress(&mut raw, &mut br, 4096, &params, dict, 1).unwrap();
    raw.reset_read();
    eprintln!("Compressed: {:?}", &br);
    let mut vec = Vec::<u8>::new();
    vec.extend(dict);
    super::decompress(&mut br, &mut rt, 4096, Rebox::from(vec)).unwrap();
    assert_eq!(rt.data(), raw.data());
}

#[test]
fn test_custom_dict_large() {
    let mut params = BrotliEncoderParams::default();
    params.quality = 11;
    let mut br = UnlimitedBuffer::new(&[]);
    let mut rt = UnlimitedBuffer::new(&[]);
    let mut dict = [0u8; 256];
    for (index, val) in dict[..].iter_mut().enumerate() {
        *val = index as u8;
    }
    let mut data_source = [0u8; 10823];
    let mut counter = 0usize;
    for (index, val) in data_source[..].iter_mut().enumerate() {
        *val = counter as u8;
        counter += 1;
        if counter * 10 > index {
            counter -= index / 11;
        }
    }
    eprintln!("Uncompressed: {:?}", &data_source);
    let mut raw = UnlimitedBuffer::new(&data_source);
    super::compress(&mut raw, &mut br, 4096, &params, &dict, 1).unwrap();
    raw.reset_read();
    eprintln!("Compressed: {:?}", &br);

    // Write debug files to temp directory (works on all platforms)
    if let Ok(temp_dir) = std::env::var("TMPDIR").or_else(|_| std::env::var("TEMP")) {
        let _ = std::fs::File::create(format!("{}/compressed.br", temp_dir))
            .and_then(|mut f| f.write_all(&br.data));
        let _ = std::fs::File::create(format!("{}/compressed.dict", temp_dir))
            .and_then(|mut f| f.write_all(&dict));
        let _ = std::fs::File::create(format!("{}/compressed.txt", temp_dir))
            .and_then(|mut f| f.write_all(&data_source));
    }

    let mut vec = Vec::<u8>::new();
    vec.extend(dict);
    super::decompress(&mut br, &mut rt, 4096, Rebox::from(vec)).unwrap();
    assert_eq!(rt.data(), raw.data());
}

#[test]
fn test_custom_dict_medium() {
    let mut params = BrotliEncoderParams::default();
    params.quality = 11;
    let mut br = UnlimitedBuffer::new(&[]);
    let mut rt = UnlimitedBuffer::new(&[]);
    let mut dict = [0u8; 256];
    for (index, val) in dict[..].iter_mut().enumerate() {
        *val = index as u8;
    }
    let mut data_source = [0u8; 323];
    let mut counter = 0usize;
    for (index, val) in data_source[..].iter_mut().enumerate() {
        *val = counter as u8;
        counter += 1;
        if counter * 10 > index {
            counter -= index / 11;
            counter = counter.wrapping_sub(43);
            counter = usize::from(counter as u8);
        }
    }
    eprintln!("Uncompressed: {:?}", &data_source[72..]);
    let new_data_source = [
        148, 100, 52, 4, 5, 6, 7, 214, 165, 116, 67, 18, 225, 176, 127, 78, 29, 235, 185, 135, 85,
        35, 241, 191, 141, 91, 41, 247, 196, 145, 94, 43, 248, 197, 146, 95, 44, 249, 198, 146, 94,
        42, 246, 194, 142, 90, 38, 242, 190, 138, 85, 32, 235, 182, 129, 76, 23, 226, 173, 120, 67,
        13, 215, 161, 107, 53, 255, 201, 147, 93, 39, 241, 186, 131, 76, 21, 222, 167, 112, 57, 2,
        3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 215, 158, 101, 44, 243, 186, 129, 72, 15,
        16, 17, 215, 157, 99, 41, 239, 181, 123, 65, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
        19, 215, 155, 95, 35, 231, 171, 111, 51, 247, 187, 127, 66, 5, 6, 7, 8, 9, 10, 11, 12, 13,
        14, 15, 16, 17, 18, 19, 20, 21, 22, 216, 154, 92, 29, 222, 159, 96, 33, 226, 163, 100, 37,
        230, 167, 103, 39, 231, 167, 103, 39, 231, 167, 103, 39, 231, 166, 101, 36, 227, 162, 97,
        32, 223, 158, 93, 28, 218, 152, 86, 20, 21, 22, 23, 24, 25, 26, 27, 216, 149, 82, 15, 16,
        17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 217, 149, 81, 13, 14, 15, 16, 17, 18,
        19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 217, 147, 77, 7, 8, 9, 10, 11,
    ];
    let mut raw = UnlimitedBuffer::new(&new_data_source);
    super::compress(&mut raw, &mut br, 4096, &params, &dict, 1).unwrap();
    raw.reset_read();
    eprintln!("Compressed: {:?}", &br);
    let mut vec = Vec::<u8>::new();
    vec.extend(dict);
    super::decompress(&mut br, &mut rt, 4096, Rebox::from(vec)).unwrap();
    assert_eq!(rt.data(), raw.data());
}
