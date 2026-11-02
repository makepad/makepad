#![cfg(test)]
extern crate core;
use std::io;
use core::cmp;
use super::brotli_decompressor::BrotliResult;
use super::brotli_decompressor::BrotliDecompressStream;
use super::brotli_decompressor::BrotliState;
use super::brotli_decompressor::HuffmanCode;
use super::HeapAllocator;

#[allow(unused_imports)]
use super::alloc_no_stdlib::{Allocator, SliceWrapper, SliceWrapperMut};
use std::time::Duration;
#[cfg(not(feature="disable-timer"))]
use std::time::SystemTime;

struct Buffer {
  data: Vec<u8>,
  read_offset: usize,
}
#[cfg(feature="disable-timer")]
fn now() -> Duration {
  return Duration::new(0, 0);
}
#[cfg(not(feature="disable-timer"))]
fn now() -> SystemTime {
  return SystemTime::now();
}

#[cfg(not(feature="disable-timer"))]
fn elapsed(start: SystemTime) -> (Duration, bool) {
  match start.elapsed() {
    Ok(delta) => return (delta, false),
    _ => return (Duration::new(0, 0), true),
  }
}

#[cfg(feature="disable-timer")]
fn elapsed(_start: Duration) -> (Duration, bool) {
  return (Duration::new(0, 0), true);
}


fn _write_all<OutputType>(w: &mut OutputType, buf: &[u8]) -> Result<(), io::Error>
  where OutputType: io::Write
{
  let mut total_written: usize = 0;
  while total_written < buf.len() {
    match w.write(&buf[total_written..]) {
      Err(e) => {
        match e.kind() {
          io::ErrorKind::Interrupted => continue,
          _ => return Err(e),
        }
      }
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


#[cfg(feature="benchmark")]
const NUM_BENCHMARK_ITERATIONS: usize = 1000;
#[cfg(not(feature="benchmark"))]
const NUM_BENCHMARK_ITERATIONS: usize = 2;

// option_env!("BENCHMARK_MODE").is_some()

pub fn decompress_internal<InputType, OutputType>(r: &mut InputType,
                                                  mut w: &mut OutputType,
                                                  input_buffer_limit: usize,
                                                  output_buffer_limit: usize,
                                                  benchmark_mode: bool)
                                                  -> Result<(), io::Error>
  where InputType: io::Read,
        OutputType: io::Write
{
  let mut total = Duration::new(0, 0);
  let range: usize;
  let mut timing_error: bool = false;
  if benchmark_mode {
    range = NUM_BENCHMARK_ITERATIONS;
  } else {
    range = 1;
  }
  for _i in 0..range {
    let mut brotli_state =
      BrotliState::new(HeapAllocator::<u8> { default_value: 0 },
                       HeapAllocator::<u32> { default_value: 0 },
                       HeapAllocator::<HuffmanCode> { default_value: HuffmanCode::default() });
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
            Err(e) => {
              match e.kind() {
                io::ErrorKind::Interrupted => continue,
                _ => return Err(e),
              }
            }
            Ok(size) => {
              if size == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Read EOF"));
              }
              available_in = size;
            }
          }
        }
        BrotliResult::NeedsMoreOutput => {
            if let Err(e) = _write_all(&mut w, &output.slice()[..output_offset]) {
                return Err(e)
            }
            output_offset = 0;
        }
        BrotliResult::ResultSuccess => break,
        BrotliResult::ResultFailure => panic!("FAILURE"),
      }
      let mut written: usize = 0;
      let start = now();
      result = BrotliDecompressStream(&mut available_in,
                                      &mut input_offset,
                                      &input.slice(),
                                      &mut available_out,
                                      &mut output_offset,
                                      &mut output.slice_mut(),
                                      &mut written,
                                      &mut brotli_state);

      let (delta, err) = elapsed(start);
      if err {
        timing_error = true;
      }
      total = total + delta;
      if output_offset != 0 {
        if let Err(e) = _write_all(&mut w, &output.slice()[..output_offset]) {
          return Err(e)
        }
        output_offset = 0;
        available_out = output.slice().len()
      }
    }
  }
  if timing_error {
    let _r = super::writeln0(&mut io::stderr(), "Timing error");
  } else {
    let _r = super::writeln_time(&mut io::stderr(),
                                 "Iterations; Time",
                                 range as u64,
                                 total.as_secs(),
                                 total.subsec_nanos());
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
    return ret;
  }
}
impl io::Read for Buffer {
  fn read(self: &mut Self, buf: &mut [u8]) -> io::Result<usize> {
    if self.read_offset == self.data.len() {
      self.read_offset = 0;
    }
    let bytes_to_read = cmp::min(buf.len(), self.data.len() - self.read_offset);
    if bytes_to_read > 0 {
      buf[0..bytes_to_read]
        .clone_from_slice(&self.data[self.read_offset..self.read_offset + bytes_to_read]);
    }
    self.read_offset += bytes_to_read;
    return Ok(bytes_to_read);
  }
}
impl io::Write for Buffer {
  fn write(self: &mut Self, buf: &[u8]) -> io::Result<usize> {
    if self.read_offset == self.data.len() {
      return Ok(buf.len());
    }
    self.data.extend(buf);
    return Ok(buf.len());
  }
  fn flush(self: &mut Self) -> io::Result<()> {
    return Ok(());
  }
}
#[test]
fn test_10x_10y() {
  let in_buf: [u8; 12] = [0x1b, 0x13, 0x00, 0x00, 0xa4, 0xb0, 0xb2, 0xea, 0x81, 0x47, 0x02, 0x8a];
  let mut input = Buffer::new(&in_buf);
  let mut output = Buffer::new(&[]);
  output.read_offset = 20;
  match super::decompress(&mut input, &mut output, 65536, Vec::new()) {
    Ok(_) => {}
    Err(e) => panic!("Error {:?}", e),
  }
  let mut i: usize = 0;
  while i < 10 {
    assert_eq!(output.data[i], 'X' as u8);
    assert_eq!(output.data[i + 10], 'Y' as u8);
    i += 1;
  }
  assert_eq!(output.data.len(), 20);
  assert_eq!(input.read_offset, in_buf.len());
}

#[test]
fn test_10x_10y_one_out_byte() {
  let in_buf: [u8; 12] = [0x1b, 0x13, 0x00, 0x00, 0xa4, 0xb0, 0xb2, 0xea, 0x81, 0x47, 0x02, 0x8a];
  let mut input = Buffer::new(&in_buf);
  let mut output = Buffer::new(&[]);
  output.read_offset = 20;
  match decompress_internal(&mut input, &mut output, 12, 1, false) {
    Ok(_) => {}
    Err(e) => panic!("Error {:?}", e),
  }
  let mut i: usize = 0;
  while i < 10 {
    assert_eq!(output.data[i], 'X' as u8);
    assert_eq!(output.data[i + 10], 'Y' as u8);
    i += 1;
  }
  assert_eq!(output.data.len(), 20);
  assert_eq!(input.read_offset, in_buf.len());
}
#[test]
fn test_10x_10y_byte_by_byte() {
  let in_buf: [u8; 12] = [0x1b, 0x13, 0x00, 0x00, 0xa4, 0xb0, 0xb2, 0xea, 0x81, 0x47, 0x02, 0x8a];
  let mut input = Buffer::new(&in_buf);
  let mut output = Buffer::new(&[]);
  output.read_offset = 20;
  match decompress_internal(&mut input, &mut output, 1, 1, false) {
    Ok(_) => {}
    Err(e) => panic!("Error {:?}", e),
  }
  let mut i: usize = 0;
  while i < 10 {
    assert_eq!(output.data[i], 'X' as u8);
    assert_eq!(output.data[i + 10], 'Y' as u8);
    i += 1;
  }
  assert_eq!(output.data.len(), 20);
  assert_eq!(input.read_offset, in_buf.len());
}


fn assert_decompressed_input_matches_output(input_slice: &[u8],
                                            output_slice: &[u8],
                                            input_buffer_size: usize,
                                            output_buffer_size: usize) {
  let mut input = Buffer::new(input_slice);
  let mut output = Buffer::new(&[]);
  output.read_offset = output_slice.len();
  if input_buffer_size == output_buffer_size {
    match super::decompress(&mut input, &mut output, input_buffer_size, Vec::new()) {
      Ok(_) => {}
      Err(e) => panic!("Error {:?}", e),
    }
  } else {
    match decompress_internal(&mut input,
                              &mut output,
                              input_buffer_size,
                              output_buffer_size,
                              false) {
      Ok(_) => {}
      Err(e) => panic!("Error {:?}", e),
    }
  }
  assert_eq!(output.data.len(), output_slice.len());
  assert_eq!(output.data, output_slice)
}

#[test]
fn test_intact_distance_ring_buffer0() {
  static BR:&'static[u8] = &[0x1b, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x80, 0xe3, 0xb4, 0x0d, 0x00, 0x00,
                            0x07, 0x5b, 0x26, 0x31, 0x40, 0x02, 0x00, 0xe0, 0x4e, 0x1b, 0xa1, 0x80,
                            0x20, 0x00];
  static OUT:&'static[u8] = b"himselfself";
  assert_decompressed_input_matches_output(BR,
                                           OUT,
                                           65536,
                                           65536);
}

#[test]
fn test_intact_distance_ring_buffer1() {
  static BR:&'static[u8] = &[0x1b, 0x09, 0x00, 0x00, 0x00, 0x00, 0x80, 0xe3, 0xb4, 0x0d, 0x00, 0x00,
    0x07, 0x5b, 0x26, 0x31, 0x40, 0x02, 0x00, 0xe0, 0x4e, 0x1b, 0x21, 0xa0,
    0x20, 0x00
  ];
  static OUT:&'static[u8] = b"scrollroll";
  assert_decompressed_input_matches_output(BR,
                                           OUT,
                                           65536,
                                           65536);
}

#[test]
fn test_intact_distance_ring_buffer2() {
  static BR:&'static[u8] = &[0x1b, 0x0f, 0x00, 0x00, 0x00, 0x00, 0x80, 0xe3, 0xb4, 0x0d, 0x00, 0x00,
    0x07, 0x5b, 0x26, 0x31, 0x40, 0x02, 0x00, 0xe0, 0x4e, 0x1b, 0x41, 0x80,
    0x20, 0x50, 0x10, 0x24, 0x08, 0x06];
  static OUT:&'static[u8] = b"leftdatadataleft";
  assert_decompressed_input_matches_output(BR,
                                           OUT,
                                           65536,
                                           65536);
}
