extern crate brotli;
#[cfg(not(feature = "std"))]
fn main() {
    panic!("For no-stdlib examples please see the tests")
}
#[cfg(feature = "std")]
fn main() {
    use std::io;
    use std::io::{Read, Write};
    let stdout = &mut io::stdout();
    {
        let mut writer = brotli::CompressorWriter::new(stdout, 4096, 11, 22);
        let mut buf = [0u8; 4096];
        loop {
            match io::stdin().read(&mut buf[..]) {
                Err(e) => {
                    if let io::ErrorKind::Interrupted = e.kind() {
                        continue;
                    }
                    panic!("{}", e);
                }
                Ok(size) => {
                    if size == 0 {
                        match writer.flush() {
                            Err(e) => {
                                if let io::ErrorKind::Interrupted = e.kind() {
                                    continue;
                                }
                                panic!("{}", e)
                            }
                            Ok(_) => break,
                        }
                    }
                    match writer.write_all(&buf[..size]) {
                        Err(e) => panic!("{}", e),
                        Ok(_) => {}
                    }
                }
            }
        }
    }
}
