// Every Read, Write and BufRead method reaches the reader or writer behind `&mut`, `Box` and
// `dyn`, as rustc's forwarding impls pass each one on, and std's writers take every buffer of a
// vectored write: the in-memory ones, BufWriter, LineWriter and stdout, and on unix one writev(2)
// for a file, a pipe, a child's stdin and a Unix socket. Relax's std passed on `read` / `write`
// alone and gave these writers the trait's default, which writes the first non-empty buffer, so a
// `Box<dyn Write>` over stdout printed a vectored write's first buffer only. A differential file
// (agree.sh): rustc's std and Relax's print the same `| ` lines.
use std::cell::RefCell;
use std::io::{self, BufRead, BufWriter, Cursor, IoSlice, IoSliceMut, LineWriter, Read, Write};
use std::rc::Rc;

/// Records which of its own methods ran.
#[derive(Clone, Default)]
struct Rec(Rc<RefCell<Vec<String>>>);

impl Rec {
    fn note(&self, what: String) {
        self.0.borrow_mut().push(what);
    }
    fn log(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.borrow_mut())
    }
}

impl Write for Rec {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.note(format!("write {}", buf.len()));
        Ok(buf.len())
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let n = bufs.iter().map(|b| b.len()).sum();
        self.note(format!("write_vectored {n}"));
        Ok(n)
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.note(format!("write_all {}", buf.len()));
        Ok(())
    }
    fn write_fmt(&mut self, args: std::fmt::Arguments<'_>) -> io::Result<()> {
        self.note(format!("write_fmt {args}"));
        Ok(())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.note("flush".to_string());
        Ok(())
    }
}

impl Read for Rec {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.note(format!("read {}", buf.len()));
        Ok(0)
    }
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.note(format!("read_vectored {}", bufs.len()));
        Ok(0)
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        self.note("read_to_end".to_string());
        buf.extend_from_slice(b"end");
        Ok(3)
    }
    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        self.note("read_to_string".to_string());
        buf.push_str("str");
        Ok(3)
    }
    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.note(format!("read_exact {}", buf.len()));
        buf.fill(b'x');
        Ok(())
    }
}

impl BufRead for Rec {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.note("fill_buf".to_string());
        Ok(b"")
    }
    fn consume(&mut self, amt: usize) {
        self.note(format!("consume {amt}"));
    }
    fn read_until(&mut self, byte: u8, buf: &mut Vec<u8>) -> io::Result<usize> {
        self.note(format!("read_until {byte}"));
        buf.extend_from_slice(b"until");
        Ok(5)
    }
    fn skip_until(&mut self, byte: u8) -> io::Result<usize> {
        self.note(format!("skip_until {byte}"));
        Ok(4)
    }
    fn read_line(&mut self, buf: &mut String) -> io::Result<usize> {
        self.note("read_line".to_string());
        buf.push_str("line");
        Ok(4)
    }
}

fn drive_write<W: Write + ?Sized>(w: &mut W) -> usize {
    let n = w.write_vectored(&[IoSlice::new(b"ab"), IoSlice::new(b""), IoSlice::new(b"cde")]).unwrap();
    w.write_all(b"xyz").unwrap();
    write!(w, "{}-{}", 1, 2).unwrap();
    w.flush().unwrap();
    n
}

fn drive_read<R: Read + ?Sized>(r: &mut R) -> String {
    let (mut a, mut b) = ([0u8; 2], [0u8; 3]);
    let n = r.read_vectored(&mut [IoSliceMut::new(&mut a), IoSliceMut::new(&mut b)]).unwrap();
    let mut s = String::new();
    r.read_to_string(&mut s).unwrap();
    let mut v = Vec::new();
    r.read_to_end(&mut v).unwrap();
    let mut e = [0u8; 2];
    r.read_exact(&mut e).unwrap();
    format!("{n} {s} {} {}", String::from_utf8_lossy(&v), String::from_utf8_lossy(&e))
}

fn drive_buf<B: BufRead + ?Sized>(r: &mut B) -> String {
    let mut v = Vec::new();
    let a = r.read_until(b'\n', &mut v).unwrap();
    let b = r.skip_until(b';').unwrap();
    let mut s = String::new();
    let c = r.read_line(&mut s).unwrap();
    format!("{a} {} {b} {c} {s}", String::from_utf8_lossy(&v))
}

#[test]
fn a01_references_and_boxes_pass_every_method_on() {
    let rec = Rec::default();
    let mut own = rec.clone();
    let n = drive_write(&mut &mut own);
    println!("| &mut W writes: {n} {:?}", rec.log());
    let n = drive_write(&mut Box::new(rec.clone()));
    println!("| Box<W> writes: {n} {:?}", rec.log());
    let mut d: Box<dyn Write> = Box::new(rec.clone());
    let n = drive_write(&mut d);
    println!("| Box<dyn Write> writes: {n} {:?}", rec.log());
    let mut own = rec.clone();
    let mut d: &mut dyn Write = &mut own;
    let n = drive_write(&mut d);
    println!("| &mut dyn Write writes: {n} {:?}", rec.log());
    let got = drive_read(&mut &mut own);
    println!("| &mut R reads: {got} {:?}", rec.log());
    let mut d: Box<dyn Read> = Box::new(rec.clone());
    let got = drive_read(&mut d);
    println!("| Box<dyn Read> reads: {got} {:?}", rec.log());
    let got = drive_buf(&mut &mut own);
    println!("| &mut B buffers: {got} {:?}", rec.log());
    let mut d: Box<dyn BufRead> = Box::new(rec.clone());
    let got = drive_buf(&mut d);
    println!("| Box<dyn BufRead> buffers: {got} {:?}", rec.log());
}

fn parts() -> [IoSlice<'static>; 3] {
    [IoSlice::new(b"ab"), IoSlice::new(b""), IoSlice::new(b"cde")]
}

#[test]
fn a02_in_memory_writers_take_every_buffer() {
    let mut v: Box<dyn Write> = Box::new(Vec::new());
    println!("| Box<dyn Write> over Vec: {}", v.write_vectored(&parts()).unwrap());
    let mut four = [0u8; 4];
    let mut s: &mut [u8] = &mut four;
    let n = s.write_vectored(&parts()).unwrap();
    println!("| &mut [u8] of 4: {n} {:?}", String::from_utf8_lossy(&four));
    let mut c = Cursor::new(Vec::new());
    c.set_position(1);
    let n = c.write_vectored(&parts()).unwrap();
    println!("| Cursor<Vec>: {n} {:?} at {}", c.get_ref(), c.position());
    let mut backing = vec![b'_'; 2];
    let mut c = Cursor::new(&mut backing);
    let n = c.write_vectored(&parts()).unwrap();
    println!("| Cursor<&mut Vec>: {n} {:?}", String::from_utf8_lossy(c.get_ref()));
    let mut four = [0u8; 4];
    let mut c = Cursor::new(&mut four[..]);
    let n = c.write_vectored(&parts()).unwrap();
    println!("| Cursor<&mut [u8]> of 4: {n} at {}", c.position());
    let mut c = Cursor::new([0u8; 3]);
    let n = c.write_vectored(&parts()).unwrap();
    println!("| Cursor<[u8; 3]>: {n} {:?}", String::from_utf8_lossy(c.get_ref()));
    let mut c = Cursor::new(vec![0u8; 6].into_boxed_slice());
    let n = c.write_vectored(&parts()).unwrap();
    println!("| Cursor<Box<[u8]>> of 6: {n} {:?}", String::from_utf8_lossy(c.get_ref()));
    println!("| sink: {}", io::sink().write_vectored(&parts()).unwrap());
    println!("| empty: {}", io::empty().write_vectored(&parts()).unwrap());
}

#[test]
fn a03_buffered_writers() {
    let two = [IoSlice::new(b"ab"), IoSlice::new(b"cd"), IoSlice::new(b"ef")];
    // over a vectored writer: buffered while they fit, then one vectored write
    let mut w = BufWriter::with_capacity(4, Vec::new());
    let a = w.write_vectored(&two).unwrap();
    let b = w.write_vectored(&[IoSlice::new(b"g"), IoSlice::new(b"h")]).unwrap();
    let buffered = w.buffer().to_vec();
    println!("| BufWriter over Vec: {a} {b} {buffered:?} {:?}", String::from_utf8_lossy(&w.into_inner().unwrap()));
    // over any other writer: buffer after buffer while it fits
    let rec = Rec::default();
    let mut w = BufWriter::with_capacity(4, rec.clone());
    let a = w.write_vectored(&two).unwrap();
    let b = w.write_vectored(&two[2..]).unwrap();
    w.flush().unwrap();
    println!("| BufWriter over a plain writer: {a} {b} {:?}", rec.log());
    let mut w = BufWriter::with_capacity(4, rec.clone());
    let a = w.write_vectored(&[IoSlice::new(b""), IoSlice::new(b"abcdef")]).unwrap();
    println!("| BufWriter, a first buffer as large as it: {a} {:?}", rec.log());
    // a LineWriter writes the complete lines in one vectored write and buffers the rest
    let mut w = LineWriter::new(Vec::new());
    let a = w.write_vectored(&[IoSlice::new(b"a"), IoSlice::new(b"b\n"), IoSlice::new(b"c")]).unwrap();
    let after = String::from_utf8_lossy(w.get_ref()).into_owned();
    let b = w.write_vectored(&[IoSlice::new(b"d")]).unwrap();
    let c = w.write_vectored(&[IoSlice::new(b"e\nf"), IoSlice::new(b"g")]).unwrap();
    println!("| LineWriter over Vec: {a} {after:?} {b} {c} {:?}", String::from_utf8_lossy(w.get_ref()));
    let mut w = LineWriter::new(rec.clone());
    let a = w.write_vectored(&[IoSlice::new(b""), IoSlice::new(b"a\nb"), IoSlice::new(b"c\n")]).unwrap();
    w.flush().unwrap();
    println!("| LineWriter over a plain writer: {a} {:?}", rec.log());
}

#[test]
fn a04_stdout_takes_every_buffer() {
    let mut out: Box<dyn Write> = Box::new(io::stdout());
    let n = out.write_vectored(&[IoSlice::new(b"| stdout a"), IoSlice::new(b"b\n")]).unwrap();
    println!("| Box<dyn Write> over stdout took {n}");
    let mut lock = io::stdout().lock();
    let n = lock.write_vectored(&[IoSlice::new(b"| lock a"), IoSlice::new(b""), IoSlice::new(b"b\n"), IoSlice::new(b"| lock tail ")]).unwrap();
    lock.flush().unwrap();
    drop(lock);
    println!("| took {n}");
}

#[cfg(unix)]
#[test]
fn a05_unix_descriptors_write_every_buffer_at_once() {
    use std::os::unix::net::UnixStream;
    use std::process::{Command, Stdio};
    let (mut r, w) = io::pipe().unwrap();
    let mut w: Box<dyn Write> = Box::new(w);
    let n = w.write_vectored(&parts()).unwrap();
    drop(w);
    let mut s = String::new();
    r.read_to_string(&mut s).unwrap();
    println!("| pipe: {n} {s}");

    let path = std::env::temp_dir().join(format!("os_io_vectored_{}", std::process::id()));
    let mut f = std::fs::File::create(&path).unwrap();
    let a = f.write_vectored(&parts()).unwrap();
    let b = (&f).write_vectored(&parts()).unwrap();
    // one byte per buffer: a call takes the OS's limit of them (1024 on Linux and Apple targets)
    let bytes = [b'.'; 1100];
    let many: Vec<IoSlice> = bytes.chunks(1).map(IoSlice::new).collect();
    let c = f.write_vectored(&many).unwrap();
    drop(f);
    let back = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    println!("| file: {a} {b} {c} {:?} {}", String::from_utf8_lossy(&back[..10]), back.len());

    let (mut x, mut y) = UnixStream::pair().unwrap();
    let a = x.write_vectored(&parts()).unwrap();
    let b = (&x).write_vectored(&parts()).unwrap();
    let mut got = [0u8; 10];
    y.read_exact(&mut got).unwrap();
    println!("| unix stream: {a} {b} {}", String::from_utf8_lossy(&got));

    let mut child = Command::new("cat").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let a = stdin.write_vectored(&parts()).unwrap();
    let b = (&stdin).write_vectored(&parts()).unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    println!("| child stdin: {a} {b} {}", String::from_utf8_lossy(&out.stdout));
}
