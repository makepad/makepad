// std-os lane: std::io (differential: os_diff.sh)
use std::io::{self, BufRead, BufReader, BufWriter, Cursor, ErrorKind, LineWriter, Read, Seek, SeekFrom, Write};

#[test]
fn a01_errors() {
    let e = io::Error::from_raw_os_error(2);
    println!("| {} | {:?} | {:?} | {:?}", e, e, e.kind(), e.raw_os_error());
    for code in [1, 4, 13, 17, 20, 21, 22, 28, 32, 9999].iter() {
        let e = io::Error::from_raw_os_error(*code);
        println!("| {} {:?} {}", code, e.kind(), e);
    }
    let e = io::Error::new(ErrorKind::Other, "custom text");
    println!("| {} | {:?} | {:?}", e, e, e.get_ref().map(|x| x.to_string()));
    let e = io::Error::other(String::from("owned"));
    println!("| {} | {:?}", e, e.kind());
    let e: io::Error = ErrorKind::NotFound.into();
    println!("| {} | {:?}", e, e);
    let kinds = [ErrorKind::NotFound, ErrorKind::PermissionDenied, ErrorKind::WouldBlock, ErrorKind::TimedOut, ErrorKind::UnexpectedEof, ErrorKind::InvalidData, ErrorKind::Other, ErrorKind::Interrupted, ErrorKind::Unsupported, ErrorKind::AlreadyExists];
    for k in kinds.iter() {
        println!("| {:?} = {}", k, k);
    }
    let mut buf = [0u8; 4];
    let e = (&b"ab"[..]).read_exact(&mut buf).unwrap_err();
    println!("| {} | {:?}", e, e);
    let e = String::from_utf8(vec![0xff]).map_err(|_| ());
    println!("| {:?}", e);
    let mut s = String::new();
    let r = (&b"ok\xff"[..]).read_to_string(&mut s);
    println!("| {:?} {:?}", r.map_err(|e| e.to_string()), s);
}

#[test]
fn a02_read_write() {
    let mut v: Vec<u8> = Vec::new();
    write!(v, "x={} y={:?}", 5, "q").unwrap();
    writeln!(v, " z").unwrap();
    v.write_all(b"tail").unwrap();
    println!("| {:?}", String::from_utf8(v.clone()).unwrap());
    let mut r = &v[..];
    let mut s = String::new();
    r.read_to_string(&mut s).unwrap();
    println!("| {:?}", s);
    let bytes: Vec<u8> = (&b"abc"[..]).bytes().map(|b| b.unwrap()).collect();
    println!("| {:?}", bytes);
    let mut t = (&b"0123456789"[..]).take(4);
    let mut out = Vec::new();
    t.read_to_end(&mut out).unwrap();
    println!("| {:?} {}", out, t.limit());
    let mut ch = (&b"ab"[..]).chain(&b"cd"[..]);
    let mut out = String::new();
    ch.read_to_string(&mut out).unwrap();
    println!("| {}", out);
    let mut dst = [0u8; 3];
    let mut w: &mut [u8] = &mut dst;
    println!("| {:?} {:?}", w.write(b"abcdef"), dst);
    let mut sink = io::sink();
    println!("| {:?} {:?}", io::copy(&mut &b"hello"[..], &mut sink), io::empty().read(&mut [0u8; 4]));
    let mut buf = Vec::new();
    println!("| {:?} {:?}", io::copy(&mut &b"hello"[..], &mut buf), buf);
}

#[test]
fn a03_bufread() {
    let data = b"line one\nline two\r\nthree\n\nlast";
    let r = BufReader::with_capacity(4, &data[..]);
    for l in r.lines() {
        println!("| {:?}", l.unwrap());
    }
    let mut r = BufReader::new(&data[..]);
    let mut s = String::new();
    println!("| {:?} {:?}", r.read_line(&mut s), s);
    let mut v = Vec::new();
    println!("| {:?} {:?}", r.read_until(b'o', &mut v), v);
    println!("| {:?}", r.buffer().len());
    let parts: Vec<Vec<u8>> = BufRead::split(&b"a,b,,c"[..], b',').map(|x| x.unwrap()).collect();
    println!("| {:?}", parts);
    let mut c = Cursor::new(b"hello world".to_vec());
    let mut five = [0u8; 5];
    c.read_exact(&mut five).unwrap();
    println!("| {:?} {}", five, c.position());
    println!("| {:?} {:?} {:?}", c.seek(SeekFrom::End(-3)), c.seek(SeekFrom::Current(1)), c.seek(SeekFrom::Current(-100)).map_err(|e| e.to_string()));
    c.seek(SeekFrom::Start(20)).unwrap();
    c.write_all(b"!").unwrap();
    println!("| {:?}", c.get_ref());
    c.set_position(0);
    let mut l = String::new();
    c.read_line(&mut l).unwrap();
    println!("| {:?} {:?}", l, c.stream_position());
    let mut arr = [0u8; 4];
    let mut c2 = Cursor::new(&mut arr[..]);
    println!("| {:?} {:?}", c2.write(b"abcdef"), c2.write(b"x"));
    println!("| {:?}", arr);
}

#[test]
fn a04_bufwriter() {
    let mut w = BufWriter::with_capacity(8, Vec::new());
    w.write_all(b"abc").unwrap();
    println!("| {} {:?}", w.get_ref().len(), w.buffer());
    w.write_all(b"defghij").unwrap();
    println!("| {} {}", w.get_ref().len(), w.buffer().len());
    w.flush().unwrap();
    println!("| {:?}", String::from_utf8(w.into_inner().unwrap()).unwrap());
    let mut lw = LineWriter::new(Vec::new());
    lw.write_all(b"ab").unwrap();
    println!("| {}", lw.get_ref().len());
    lw.write_all(b"c\nde").unwrap();
    println!("| {:?}", lw.get_ref());
    lw.flush().unwrap();
    println!("| {:?}", lw.get_ref());
}

#[test]
fn a05_stdout() {
    let out = io::stdout();
    let mut lock = out.lock();
    writeln!(lock, "| locked line {}", 1).unwrap();
    drop(lock);
    print!("| partial ");
    println!("done");
    io::stdout().write_all(b"| raw bytes\n").unwrap();
    io::stdout().flush().unwrap();
    eprintln!("to stderr (not compared)");
    println!("| {:?} {:?}", io::stdout(), io::stdin());
}
