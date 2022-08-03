extern crate jpeg_decoder as jpeg;
extern crate png as png;
use std::fs::File;
use std::io::BufReader;

fn main() {
    for i in 0..10{
        let time = std::time::Instant::now();
        let file = File::open("test.jpg").expect("failed to open file");
        let mut decoder = jpeg::Decoder::new(BufReader::new(file));
        let pixels = decoder.decode().expect("failed to decode image");
        let metadata = decoder.info().unwrap();
        println!("Profile time {} ms", (time.elapsed().as_nanos() as f64) / 1000000f64);
    }
    for i in 0..10{
        let time = std::time::Instant::now();
        let decoder = png::Decoder::new(File::open("test.png").unwrap());
        let mut reader = decoder.read_info().unwrap();
        // Allocate the output buffer.
        let mut buf = vec![0; reader.output_buffer_size()];
        // Read the next frame. An APNG might contain multiple frames.
        let info = reader.next_frame(&mut buf).unwrap();
        println!("Profile time {} ms", (time.elapsed().as_nanos() as f64) / 1000000f64);
    }
}