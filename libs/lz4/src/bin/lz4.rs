//! `makepad-lz4 [-d] <in|-> <out|->`: an LZ4 frame from a file or stdin
//! to a file or stdout, and back. What local/wm-dyn/pack.py uses to turn
//! the super-app's tarballs into the frames the phone streams through
//! `makepad_tar` (no whole-archive buffer, see apps/wm/src/dylib_host.rs).

use makepad_lz4::{FrameDecoder, FrameEncoder};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::process::ExitCode;

const CHUNK: usize = 4 << 20;

fn open_in(path: &str) -> io::Result<(Box<dyn Read>, Option<u64>)> {
    if path == "-" {
        return Ok((Box::new(io::stdin().lock()), None));
    }
    let file = File::open(path)?;
    let len = file.metadata().map(|m| m.len()).ok();
    Ok((Box::new(BufReader::with_capacity(CHUNK, file)), len))
}

fn open_out(path: &str) -> io::Result<Box<dyn Write>> {
    if path == "-" {
        return Ok(Box::new(io::stdout().lock()));
    }
    Ok(Box::new(BufWriter::with_capacity(CHUNK, File::create(path)?)))
}

fn run(decompress: bool, input: &str, output: &str) -> io::Result<(u64, u64)> {
    let (mut source, len) = open_in(input)?;
    let sink = open_out(output)?;
    let mut buf = vec![0u8; CHUNK];
    let (mut read, mut wrote) = (0u64, 0u64);
    if decompress {
        let mut dec = FrameDecoder::new(&mut source)?;
        let mut sink = sink;
        loop {
            let n = dec.read(&mut buf)?;
            if n == 0 {
                break;
            }
            sink.write_all(&buf[..n])?;
            wrote += n as u64;
        }
        sink.flush()?;
        read = len.unwrap_or(0);
    } else {
        let mut enc = match len {
            Some(len) => FrameEncoder::with_content_size(sink, len),
            None => FrameEncoder::new(sink),
        };
        loop {
            let n = source.read(&mut buf)?;
            if n == 0 {
                break;
            }
            enc.write_all(&buf[..n])?;
            read += n as u64;
        }
        let mut sink = enc.finish()?;
        sink.flush()?;
        wrote = 0;
    }
    Ok((read, wrote))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (decompress, rest) = match args.first().map(String::as_str) {
        Some("-d") => (true, &args[1..]),
        _ => (false, &args[..]),
    };
    if rest.len() != 2 {
        eprintln!("usage: makepad-lz4 [-d] <in|-> <out|->");
        return ExitCode::from(2);
    }
    match run(decompress, &rest[0], &rest[1]) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("makepad-lz4: {}: {e}", if decompress { "decompress" } else { "compress" });
            ExitCode::FAILURE
        }
    }
}
