// The writes a LineWriter (stdout's) hands its writer: one per completed line, its buffered start
// and its end together, as real std's write_all makes them. `println!("x {}", v)` is then one
// write, and a diagnostic the host writes to a stderr merged with stdout cannot land inside it.
use std::io::Write;

struct Rec(Vec<String>);

impl Write for Rec {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.push(String::from_utf8_lossy(b).into_owned());
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn main() {
    // runtime values: rustc folds literal arguments into the format string
    let (a, b) = (std::env::args().count(), "two".len() - 1);
    let mut w = std::io::LineWriter::new(Rec(Vec::new()));
    writeln!(w, "LINE a={} b={}", a, b).unwrap();
    write!(w, "part ").unwrap();
    writeln!(w, "{}", a + b).unwrap();
    write!(w, "two\nlines {}\ntail", a).unwrap();
    w.flush().unwrap();
    println!("line writer writes {:?}", w.get_ref().0);
}
