// io::repeat / Repeat, Empty as a writer and seeker, &Sink, IoSlice / IoSliceMut (new, deref,
// advance, advance_slices), the default read_vectored / write_vectored, the adapters' derived
// Debug (Take, Chain, Bytes, Lines, Split), Seek for Take and BufRead for Chain (real std).
use std::io::{self, BufRead, Cursor, IoSlice, IoSliceMut, Read, Seek, SeekFrom, Write};

#[test]
fn a01_repeat_empty_sink() {
    let mut buf = [0u8; 5];
    let n = io::repeat(7).read(&mut buf).unwrap();
    println!("| repeat {} {:?}", n, buf);
    let mut taken = Vec::new();
    io::repeat(b'z').take(4).read_to_end(&mut taken).unwrap();
    println!("| take {:?}", String::from_utf8(taken).unwrap());
    let mut v = Vec::new();
    println!("| repeat read_to_end {:?}", io::repeat(1).read_to_end(&mut v).map_err(|e| e.kind()));
    let mut e = io::empty();
    println!("| empty write {:?} seek {:?} flush {:?}", e.write(b"abc").map_err(|e| e.kind()), e.seek(SeekFrom::End(5)).map_err(|e| e.kind()), e.flush().is_ok());
    let s = io::sink();
    let mut r = &s;
    println!("| &sink {:?} {:?}", r.write(b"xyz").map_err(|e| e.kind()), io::copy(&mut io::repeat(0).take(1000), &mut io::sink()).map_err(|e| e.kind()));
    println!("| debug {:?} {:?}", io::empty(), io::sink());
}

#[test]
fn a02_io_slices() {
    let a = [1u8, 2, 3];
    let b = [4u8, 5];
    let c = [6u8, 7, 8, 9];
    let mut slices = [IoSlice::new(&a), IoSlice::new(&b), IoSlice::new(&c)];
    println!("| deref {} {:?}", slices[0].len(), &*slices[1]);
    let mut bufs: &mut [IoSlice] = &mut slices;
    IoSlice::advance_slices(&mut bufs, 4);
    println!("| advanced {} first {:?}", bufs.len(), &*bufs[0]);
    let mut one = IoSlice::new(&c);
    one.advance(2);
    println!("| one {:?} dbg {:?}", &*one, one);
    let mut out = Vec::new();
    let n = out.write_vectored(&[IoSlice::new(&[]), IoSlice::new(&a), IoSlice::new(&b)]).unwrap();
    println!("| write_vectored {} {:?}", n, out);
    let mut x = [0u8; 2];
    let mut y = [0u8; 3];
    let mut src: &[u8] = &[9, 8, 7, 6, 5];
    let n = src.read_vectored(&mut [IoSliceMut::new(&mut x), IoSliceMut::new(&mut y)]).unwrap();
    println!("| read_vectored {} {:?} {:?}", n, x, y);
    let mut m1 = [1u8; 3];
    let mut m2 = [2u8; 3];
    let mut ms = [IoSliceMut::new(&mut m1), IoSliceMut::new(&mut m2)];
    let mut mb: &mut [IoSliceMut] = &mut ms;
    IoSliceMut::advance_slices(&mut mb, 5);
    mb[0][0] = 99;
    println!("| mut advanced {} {:?}", mb.len(), &*mb[0]);
}

#[test]
fn a03_adapters_debug() {
    use std::io::BufRead;
    // relax issue 99: a derived Debug over a `?Sized` reader's Take field (zip's ZipFileReader),
    // here for an unsized reader
    #[derive(Debug)]
    enum Reader<'a, R: Read + ?Sized> {
        Raw(io::Take<&'a mut R>),
        None,
    }
    trait DebugRead: Read + std::fmt::Debug {}
    impl<T: Read + std::fmt::Debug> DebugRead for T {}
    let mut b = [0u8; 2];
    let mut cur = io::Cursor::new(vec![1u8, 2, 3, 4, 5]);
    let r: &mut dyn DebugRead = &mut cur;
    let mut raw: Reader<'_, dyn DebugRead> = Reader::Raw(r.take(3));
    if let Reader::Raw(t) = &mut raw {
        t.read(&mut b).unwrap();
    }
    println!("| reader {:?} {:?}", raw, Reader::<dyn DebugRead>::None);
    // real std's derived Debug of the adapters: Take's len (the limit as set) and limit (left)
    let mut t = io::Cursor::new(vec![1u8, 2, 3, 4, 5]).take(3);
    println!("| take {:?}", t);
    t.read(&mut b).unwrap();
    println!("| take {:?}", t);
    t.set_limit(10);
    println!("| take {:?} {}", t, t.limit());
    let mut c = (&b"ab"[..]).chain(&b"cd"[..]);
    println!("| chain {:?}", c);
    let mut three = [0u8; 3];
    let n = c.read(&mut three).unwrap();
    println!("| chain {} {:?}", n, c);
    println!("| bytes {:?}", (&b"xy"[..]).bytes());
    println!("| lines {:?}", io::Cursor::new(&b"l1\nl2"[..]).lines());
    println!("| split {:?}", io::Cursor::new(&b"a,b"[..]).split(b','));
    println!("| pretty {}", format!("{:#?}", io::empty().take(7)).replace('\n', "/"));
}

#[test]
fn a04_take_seek() {
    // real std's Seek for Take: a position counts from where the Take began, within its len
    let show = |r: io::Result<u64>| format!("{:?}", r.map_err(|e| (e.kind(), e.to_string())));
    let mut inner = io::Cursor::new(vec![1u8, 2, 3, 4, 5, 6, 7]);
    inner.set_position(1);
    let mut t = inner.take(4);
    let mut b = [0u8; 1];
    t.read(&mut b).unwrap();
    println!("| at {:?} {:?}", t.stream_position(), t);
    for to in [SeekFrom::Current(1), SeekFrom::Start(0), SeekFrom::End(-1), SeekFrom::Current(9), SeekFrom::Current(-9), SeekFrom::End(1), SeekFrom::Start(4), SeekFrom::End(-4)] {
        let r = show(t.seek(to));
        let mut x = [0u8; 1];
        let n = t.read(&mut x).unwrap();
        println!("| seek {:?} {} read {} {:?} {:?}", to, r, n, x, t);
    }
    let rel = |t: &mut io::Take<io::Cursor<Vec<u8>>>, o: i64| format!("{:?}", t.seek_relative(o).map_err(|e| e.kind()));
    println!("| relative {} {} {} {:?}", rel(&mut t, 1), rel(&mut t, 9), rel(&mut t, -9), t);
    t.rewind().unwrap();
    println!("| rewound {:?} {:?}", t.stream_position(), t);
    t.set_limit(2);
    println!("| set {} {} {:?}", show(t.seek(SeekFrom::End(0))), show(t.seek(SeekFrom::Start(3))), t);
}

// A BufRead that logs its calls: its read_until is its own; `empty_once` makes the first
// fill_buf come back empty although data follows.
struct Logged {
    data: Vec<u8>,
    pos: usize,
    empty_once: bool,
}

impl Read for Logged {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let n = {
            let a = self.fill_buf()?;
            let n = a.len().min(b.len());
            b[..n].copy_from_slice(&a[..n]);
            n
        };
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for Logged {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.empty_once {
            self.empty_once = false;
            println!("| first fill_buf empty");
            return Ok(&[]);
        }
        Ok(&self.data[self.pos..])
    }
    fn consume(&mut self, n: usize) {
        println!("| first consume {n}");
        self.pos += n;
    }
    fn read_until(&mut self, byte: u8, buf: &mut Vec<u8>) -> io::Result<usize> {
        println!("| first read_until");
        let start = self.pos;
        while self.pos < self.data.len() {
            let c = self.data[self.pos];
            self.pos += 1;
            buf.push(c);
            if c == byte {
                break;
            }
        }
        Ok(self.pos - start)
    }
}

fn logged(data: &[u8], empty_once: bool) -> Logged {
    Logged { data: data.to_vec(), pos: 0, empty_once }
}

fn count_lines<R: BufRead>(r: R) -> usize {
    r.lines().count()
}

#[test]
fn a05_chain_bufread() {
    let chained = Cursor::new("a\nb".to_string()).chain(Cursor::new("\nc".to_string()));
    let lines: Vec<String> = chained.lines().map(Result::unwrap).collect();
    println!("| {lines:?}");
    println!("| {}", count_lines(Cursor::new("x\ny\n").chain(&b"z"[..])));
    // fill_buf and consume across the boundary: the first's rest, then the second's
    let mut c = (&b"abc"[..]).chain(&b"de"[..]);
    println!("| {:?}", c.fill_buf().map(|b| b.to_vec()));
    c.consume(2);
    println!("| {:?}", c.fill_buf().map(|b| b.to_vec()));
    c.consume(1);
    println!("| {:?}", c.fill_buf().map(|b| b.to_vec()));
    c.consume(1);
    println!("| {:?}", c.fill_buf().map(|b| b.to_vec()));
    c.consume(1);
    println!("| {:?}", c.fill_buf().map(|b| b.to_vec()));
    let mut e = (&b""[..]).chain(&b"q"[..]);
    println!("| {:?}", e.fill_buf().map(|b| b.to_vec()));
    // read_until: the first reader's own, and on into the second when it ends without the byte
    let mut c = logged(b"ab\ncd", false).chain(Cursor::new(b"ef\ngh".to_vec()));
    let mut v = Vec::new();
    println!("| {:?} {:?}", c.read_until(b'\n', &mut v), String::from_utf8_lossy(&v));
    v.clear();
    println!("| {:?} {:?}", c.read_until(b'\n', &mut v), String::from_utf8_lossy(&v));
    let mut l = String::new();
    println!("| {:?} {:?}", c.read_line(&mut l), l);
    // an empty fill_buf ends the first reader for good
    let lines: Vec<String> = logged(b"x\ny", true).chain(Cursor::new(b"z".to_vec())).lines().map(Result::unwrap).collect();
    println!("| {lines:?}");
    // split goes through Chain's read_until; skip_until through fill_buf and consume
    let parts: Vec<Vec<u8>> = logged(b"1;2", false).chain(Cursor::new(b";3".to_vec())).split(b';').map(Result::unwrap).collect();
    println!("| {parts:?}");
    let mut s = logged(b"pq", false).chain(Cursor::new(b"rs".to_vec()));
    println!("| {:?}", s.skip_until(b'r'));
    let mut rest = String::new();
    s.read_to_string(&mut rest).unwrap();
    println!("| {rest:?}");
    // a UTF-8 sequence split between the two readers, and a chain of chains
    let bytes = "x\u{e9}y\n".as_bytes();
    let mut u = (&bytes[..2]).chain(&bytes[2..]);
    let mut l = String::new();
    println!("| {:?} {:?}", u.read_line(&mut l), l);
    let nested = (&b"1\n"[..]).chain(&b"2"[..]).chain((&b"\n3"[..]).chain(&b"\n"[..]));
    let lines: Vec<String> = nested.lines().map(Result::unwrap).collect();
    println!("| {lines:?}");
}
