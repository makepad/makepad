// io::Seek for Box<S> (S: Seek + ?Sized), forwarding seek, rewind, stream_position and
// seek_relative to the boxed value, and the same full forwarding for `&mut S`. image 0.25's
// GenericReader wraps `BufReader<Box<dyn ReadSeek>>` and forwards its own Seek impl to it
// (src/hooks.rs: `self.0.seek(pos)`, `self.0.rewind()`, `self.0.stream_position()`).
use std::io::{BufRead, BufReader, Cursor, Read, Result, Seek, SeekFrom};

// image's shape
trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

struct GenericReader<'a>(BufReader<Box<dyn ReadSeek + 'a>>);
impl Read for GenericReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.0.read(buf)
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        self.0.read_to_end(buf)
    }
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        self.0.read_exact(buf)
    }
}
impl BufRead for GenericReader<'_> {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        self.0.fill_buf()
    }
    fn consume(&mut self, amt: usize) {
        self.0.consume(amt)
    }
}
impl Seek for GenericReader<'_> {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.0.seek(pos)
    }
    fn rewind(&mut self) -> Result<()> {
        self.0.rewind()
    }
    fn stream_position(&mut self) -> Result<u64> {
        self.0.stream_position()
    }
}

#[test]
fn buf_reader_over_a_boxed_reader_seeks() {
    let data: Vec<u8> = (0u8..100).collect();
    let borrowed = Cursor::new(&data[..]);
    let mut r = GenericReader(BufReader::with_capacity(16, Box::new(borrowed)));
    let mut four = [0u8; 4];
    r.read_exact(&mut four).unwrap();
    assert_eq!(four, [0, 1, 2, 3]);
    assert_eq!(r.stream_position().unwrap(), 4);
    assert_eq!(r.seek(SeekFrom::Start(40)).unwrap(), 40);
    r.read_exact(&mut four).unwrap();
    assert_eq!(four, [40, 41, 42, 43]);
    assert_eq!(r.seek(SeekFrom::Current(-2)).unwrap(), 42);
    assert_eq!(r.fill_buf().unwrap()[0], 42);
    assert_eq!(r.seek(SeekFrom::End(-1)).unwrap(), 99);
    let mut rest = Vec::new();
    r.read_to_end(&mut rest).unwrap();
    assert_eq!(rest, [99]);
    r.rewind().unwrap();
    assert_eq!(r.stream_position().unwrap(), 0);
    r.read_exact(&mut four).unwrap();
    assert_eq!(four, [0, 1, 2, 3]);
    r.seek_relative(6).unwrap();
    r.read_exact(&mut four).unwrap();
    assert_eq!(four, [10, 11, 12, 13]);
    assert_eq!(r.seek(SeekFrom::Current(-100)).is_err(), true);
}

// which of its four methods a seeker was asked for
#[derive(Default)]
struct Counting {
    pos: u64,
    seeks: u32,
    rewinds: u32,
    positions: u32,
    relatives: u32,
}
impl Seek for Counting {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.seeks += 1;
        self.pos = match pos {
            SeekFrom::Start(n) => n,
            SeekFrom::Current(d) => (self.pos as i64 + d) as u64,
            SeekFrom::End(d) => (1000 + d) as u64,
        };
        Ok(self.pos)
    }
    fn rewind(&mut self) -> Result<()> {
        self.rewinds += 1;
        self.pos = 0;
        Ok(())
    }
    fn stream_position(&mut self) -> Result<u64> {
        self.positions += 1;
        Ok(self.pos)
    }
    fn seek_relative(&mut self, offset: i64) -> Result<()> {
        self.relatives += 1;
        self.pos = (self.pos as i64 + offset) as u64;
        Ok(())
    }
}

fn drive<S: Seek>(mut s: S) -> (u64, u64) {
    let a = s.seek(SeekFrom::End(-10)).unwrap();
    s.rewind().unwrap();
    s.seek_relative(7).unwrap();
    s.seek_relative(-2).unwrap();
    (a, s.stream_position().unwrap())
}

#[test]
fn box_and_mut_ref_forward_every_seek_method() {
    let mut boxed: Box<Counting> = Box::new(Counting::default());
    assert_eq!(drive(&mut boxed), (990, 5));
    assert_eq!((boxed.seeks, boxed.rewinds, boxed.positions, boxed.relatives), (1, 1, 1, 2));
    assert_eq!(drive(boxed).1, 5);

    let mut dynamic: Box<dyn Seek> = Box::new(Counting::default());
    assert_eq!(dynamic.seek(SeekFrom::Start(12)).unwrap(), 12);
    assert_eq!(dynamic.stream_position().unwrap(), 12);
    dynamic.rewind().unwrap();
    assert_eq!(dynamic.seek(SeekFrom::Current(3)).unwrap(), 3);
    assert_eq!(drive(dynamic), (990, 5));

    let mut plain = Counting::default();
    assert_eq!(drive(&mut plain), (990, 5));
    assert_eq!((plain.seeks, plain.rewinds, plain.positions, plain.relatives), (1, 1, 1, 2));
    {
        let by_ref: &mut dyn Seek = &mut plain;
        assert_eq!(drive(by_ref), (990, 5));
    }
    assert_eq!((plain.seeks, plain.rewinds, plain.positions, plain.relatives), (2, 2, 2, 4));

    let mut cursor: Box<Cursor<Vec<u8>>> = Box::new(Cursor::new(vec![1, 2, 3, 4, 5]));
    assert_eq!(cursor.seek(SeekFrom::End(-2)).unwrap(), 3);
    let mut two = [0u8; 2];
    cursor.read_exact(&mut two).unwrap();
    assert_eq!(two, [4, 5]);
    cursor.rewind().unwrap();
    assert_eq!(cursor.stream_position().unwrap(), 0);
}
