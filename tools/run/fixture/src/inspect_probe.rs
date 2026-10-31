//! makepad inspect's probes, `eval`, `static` and `locals` against a live program
//! (tools/run/fixture/inspect_probe.sh): a module-private static slice, an inherent method with
//! a lifetime parameter, and a panic that parks once the trigger file exists.

mod vector {
    static FONT: &[u8] = include_bytes!("guard.rs");

    pub struct Face<'a> {
        data: &'a [u8],
    }

    #[derive(Debug)]
    pub struct ParseError;

    impl<'a> Face<'a> {
        #[inline(never)]
        pub fn parse(data: &'a [u8], index: u32) -> Result<Face<'a>, ParseError> {
            if data.len() < 4 || index > 0 {
                return Err(ParseError);
            }
            Ok(Face { data })
        }

        pub fn len(&self) -> usize {
            self.data.len()
        }
    }

    #[inline(never)]
    pub fn font() -> usize {
        Face::parse(FONT, 0).map(|f| f.len()).unwrap_or(0)
    }

    #[inline(never)]
    pub fn boom(n: usize) -> usize {
        let label = "parked";
        let numbers = vec![1u32, 2, 3];
        if n > 0 {
            panic!("boom: {} {} {:?}", n, label, numbers);
        }
        numbers.len()
    }
}

fn main() {
    let trigger = std::env::var("PROBE_TRIGGER").unwrap_or_default();
    let secs: u64 = std::env::var("PROBE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
    let mut total = 0usize;
    for _ in 0..secs * 10 {
        total = total.wrapping_add(vector::font());
        if !trigger.is_empty() && std::path::Path::new(&trigger).exists() {
            let _ = std::fs::remove_file(&trigger);
            total = total.wrapping_add(vector::boom(total));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    println!("total {}", total);
}
