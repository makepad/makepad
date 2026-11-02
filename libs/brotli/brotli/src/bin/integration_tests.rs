#![cfg(test)]
#![allow(non_upper_case_globals)]
#![allow(dead_code)]

extern crate brotli_decompressor;
extern crate core;

use core::cmp::min;
use std::io;
use std::time::Duration;
#[cfg(not(feature = "disable-timer"))]
use std::time::SystemTime;

use brotli::BrotliDecompressStream;

use super::alloc_no_stdlib::{Allocator, SliceWrapper, SliceWrapperMut};
use super::brotli::{BrotliResult, BrotliState};
use super::{HeapAllocator, Rebox};

pub struct Buffer {
    data: Vec<u8>,
    read_offset: usize,
}

#[derive(Debug)]
pub struct UnlimitedBuffer {
    pub data: Vec<u8>,
    read_offset: usize,
}

#[cfg(feature = "disable-timer")]
fn now() -> Duration {
    return Duration::new(0, 0);
}
#[cfg(not(feature = "disable-timer"))]
fn now() -> SystemTime {
    SystemTime::now()
}

#[cfg(not(feature = "disable-timer"))]
fn elapsed(start: SystemTime) -> (Duration, bool) {
    match start.elapsed() {
        Ok(delta) => (delta, false),
        _ => (Duration::new(0, 0), true),
    }
}

#[cfg(feature = "disable-timer")]
fn elapsed(_start: Duration) -> (Duration, bool) {
    return (Duration::new(0, 0), true);
}

fn _write_all<OutputType>(w: &mut OutputType, buf: &[u8]) -> Result<(), io::Error>
where
    OutputType: io::Write,
{
    let mut total_written: usize = 0;
    while total_written < buf.len() {
        match w.write(&buf[total_written..]) {
            Err(e) => match e.kind() {
                io::ErrorKind::Interrupted => continue,
                _ => return Err(e),
            },
            Ok(cur_written) => {
                if cur_written == 0 {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Write EOF"));
                }
                total_written += cur_written;
            }
        }
    }
    Ok(())
}
trait Runner {
    fn iter<Fn: FnMut()>(&mut self, cb: &mut Fn);
}

struct Passthrough {}
impl Runner for Passthrough {
    fn iter<Fn: FnMut()>(&mut self, cb: &mut Fn) {
        cb()
    }
}

fn decompress_internal<InputType, OutputType, Run: Runner>(
    r: &mut InputType,
    mut w: &mut OutputType,
    input_buffer_limit: usize,
    output_buffer_limit: usize,
    runner: &mut Run,
) -> Result<(), io::Error>
where
    InputType: io::Read,
    OutputType: io::Write,
{
    let mut total = Duration::new(0, 0);
    let mut range: usize = 0;
    let mut timing_error: bool = false;
    runner.iter(&mut || {
        range += 1;
        let mut brotli_state = BrotliState::new(
            HeapAllocator::default(),
            HeapAllocator::default(),
            HeapAllocator::default(),
        );
        let mut input = brotli_state.alloc_u8.alloc_cell(input_buffer_limit);
        let mut output = brotli_state.alloc_u8.alloc_cell(output_buffer_limit);
        let mut available_out: usize = output.slice().len();

        // let amount = try!(r.read(&mut buf));
        let mut available_in: usize = 0;
        let mut input_offset: usize = 0;
        let mut output_offset: usize = 0;
        let mut result: BrotliResult = BrotliResult::NeedsMoreInput;
        loop {
            match result {
                BrotliResult::NeedsMoreInput => {
                    input_offset = 0;
                    match r.read(input.slice_mut()) {
                        Err(e) => match e.kind() {
                            io::ErrorKind::Interrupted => continue,
                            _ => panic!("{}", e),
                        },
                        Ok(size) => {
                            if size == 0 {
                                panic!(
                                    "{:?}",
                                    io::Error::new(io::ErrorKind::UnexpectedEof, "Read EOF")
                                );
                            }
                            available_in = size;
                        }
                    }
                }
                BrotliResult::NeedsMoreOutput => {
                    _write_all(&mut w, &output.slice()[..output_offset]).unwrap();
                    output_offset = 0;
                }
                BrotliResult::ResultSuccess => break,
                BrotliResult::ResultFailure => panic!("FAILURE"),
            }
            let mut written: usize = 0;
            let start = now();
            result = BrotliDecompressStream(
                &mut available_in,
                &mut input_offset,
                input.slice(),
                &mut available_out,
                &mut output_offset,
                output.slice_mut(),
                &mut written,
                &mut brotli_state,
            );

            let (delta, err) = elapsed(start);
            if err {
                timing_error = true;
            }
            total += delta;
            if output_offset != 0 {
                _write_all(&mut w, &output.slice()[..output_offset]).unwrap();
                output_offset = 0;
                available_out = output.slice().len()
            }
        }
    });
    if timing_error {
        let _r = super::writeln0(&mut io::stderr(), "Timing error");
    } else {
        let _r = super::writeln_time(
            &mut io::stderr(),
            "Iterations; Time",
            range as u64,
            total.as_secs(),
            total.subsec_nanos(),
        );
    }
    Ok(())
}

impl Buffer {
    pub fn new(buf: &[u8]) -> Buffer {
        let mut ret = Buffer {
            data: Vec::<u8>::new(),
            read_offset: 0,
        };
        ret.data.extend(buf);
        ret
    }
}
impl UnlimitedBuffer {
    pub fn new(buf: &[u8]) -> Self {
        let mut ret = UnlimitedBuffer {
            data: Vec::<u8>::new(),
            read_offset: 0,
        };
        ret.data.extend(buf);
        ret
    }
    pub fn reset_read(&mut self) {
        self.read_offset = 0;
    }
    pub fn data(&self) -> &[u8] {
        &self.data[..]
    }
}
impl io::Read for Buffer {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.read_offset == self.data.len() {
            self.read_offset = 0;
        }
        let bytes_to_read = min(buf.len(), self.data.len() - self.read_offset);
        if bytes_to_read > 0 {
            buf[0..bytes_to_read]
                .clone_from_slice(&self.data[self.read_offset..self.read_offset + bytes_to_read]);
        }
        self.read_offset += bytes_to_read;
        Ok(bytes_to_read)
    }
}
impl io::Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.read_offset == self.data.len() {
            return Ok(buf.len());
        }
        self.data.extend(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl io::Read for UnlimitedBuffer {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let bytes_to_read = min(buf.len(), self.data.len() - self.read_offset);
        if bytes_to_read > 0 {
            buf[0..bytes_to_read]
                .clone_from_slice(&self.data[self.read_offset..self.read_offset + bytes_to_read]);
        }
        self.read_offset += bytes_to_read;
        Ok(bytes_to_read)
    }
}

impl io::Write for UnlimitedBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.data.extend(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[allow(non_snake_case)]
#[test]
fn test_roundtrip_64x() {
    let X = b'X';
    let in_buf: [u8; 64] = [
        X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X,
        X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X, X,
        X, X, X, X,
    ];

    let mut input = UnlimitedBuffer::new(&in_buf);
    let mut compressed = UnlimitedBuffer::new(&[]);
    let mut output = UnlimitedBuffer::new(&[]);
    let q: i32 = 9;
    let lgwin: i32 = 16;
    let mut params = super::brotli::enc::BrotliEncoderInitParams();
    params.quality = q;
    params.lgwin = lgwin;
    match super::compress(&mut input, &mut compressed, 65536, &params, &[], 1) {
        Ok(_) => {}
        Err(e) => panic!("Error {:?}", e),
    }
    let mut compressed_in = UnlimitedBuffer::new(&compressed.data[..]);
    match super::decompress(&mut compressed_in, &mut output, 65536, Rebox::default()) {
        Ok(_) => {}
        Err(e) => panic!("Error {:?}", e),
    }
    for i in 0..input.data.len() {
        assert_eq!(output.data[i], input.data[i]);
    }
    assert_eq!(output.data.len(), input.data.len());
    assert_eq!(input.read_offset, in_buf.len());
}

#[test]
fn test_10x_10y() {
    let in_buf: [u8; 12] = [
        0x1b, 0x13, 0x00, 0x00, 0xa4, 0xb0, 0xb2, 0xea, 0x81, 0x47, 0x02, 0x8a,
    ];
    let mut input = Buffer::new(&in_buf);
    let mut output = Buffer::new(&[]);
    output.read_offset = 20;
    match super::decompress(&mut input, &mut output, 65536, Rebox::default()) {
        Ok(_) => {}
        Err(e) => panic!("Error {:?}", e),
    }
    let mut i: usize = 0;
    while i < 10 {
        assert_eq!(output.data[i], b'X');
        assert_eq!(output.data[i + 10], b'Y');
        i += 1;
    }
    assert_eq!(output.data.len(), 20);
    assert_eq!(input.read_offset, in_buf.len());
}

#[test]
fn test_10x_10y_one_out_byte() {
    let in_buf: [u8; 12] = [
        0x1b, 0x13, 0x00, 0x00, 0xa4, 0xb0, 0xb2, 0xea, 0x81, 0x47, 0x02, 0x8a,
    ];
    let mut input = Buffer::new(&in_buf);
    let mut output = Buffer::new(&[]);
    output.read_offset = 20;
    match decompress_internal(&mut input, &mut output, 12, 1, &mut Passthrough {}) {
        Ok(_) => {}
        Err(e) => panic!("Error {:?}", e),
    }
    let mut i: usize = 0;
    while i < 10 {
        assert_eq!(output.data[i], b'X');
        assert_eq!(output.data[i + 10], b'Y');
        i += 1;
    }
    assert_eq!(output.data.len(), 20);
    assert_eq!(input.read_offset, in_buf.len());
}

#[test]
fn test_10x_10y_byte_by_byte() {
    let in_buf: [u8; 12] = [
        0x1b, 0x13, 0x00, 0x00, 0xa4, 0xb0, 0xb2, 0xea, 0x81, 0x47, 0x02, 0x8a,
    ];
    let mut input = Buffer::new(&in_buf);
    let mut output = Buffer::new(&[]);
    output.read_offset = 20;
    match decompress_internal(&mut input, &mut output, 1, 1, &mut Passthrough {}) {
        Ok(_) => {}
        Err(e) => panic!("Error {:?}", e),
    }
    let mut i: usize = 0;
    while i < 10 {
        assert_eq!(output.data[i], b'X');
        assert_eq!(output.data[i + 10], b'Y');
        i += 1;
    }
    assert_eq!(output.data.len(), 20);
    assert_eq!(input.read_offset, in_buf.len());
}

pub struct SoonErrorReader(&'static [u8], bool);
impl io::Read for SoonErrorReader {
    fn read(&mut self, data: &mut [u8]) -> io::Result<usize> {
        let first = self.1;
        self.1 = false;
        if first {
            let len = min(self.0.len(), data.len());
            data[..len].clone_from_slice(&self.0[..len]);
            return Ok(len);
        }
        Err(io::Error::new(io::ErrorKind::PermissionDenied, "err"))
    }
}
#[test]
fn test_error_returned() {
    static onetwothreefourfive: [u8; 5] = [1, 2, 3, 4, 5];
    let params = super::brotli::enc::BrotliEncoderParams::default();
    let mut erroring = SoonErrorReader(&onetwothreefourfive[..], true);
    let mut br = UnlimitedBuffer::new(&[]);
    let dict = &[];
    let err = super::compress(&mut erroring, &mut br, 4096, &params, dict, 1);
    if let Ok(_x) = err {
        panic!("Should have errored {:?}\n", err);
    }
    let mut output = UnlimitedBuffer::new(&[]);
    let mut br_copy = Buffer::new(&br.data[..]);
    assert_eq!(br.data.len(), 9);
    match decompress_internal(&mut br_copy, &mut output, 1, 1, &mut Passthrough {}) {
        Ok(_) => {}
        Err(e) => panic!("Error {:?}", e),
    }
    assert_eq!(output.data, &onetwothreefourfive[..]);
}
