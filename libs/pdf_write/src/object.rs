use makepad_fast_inflate::zlib_compress;

pub(crate) struct Objects {
    bytes: Vec<u8>,
    offsets: Vec<usize>,
}

impl Objects {
    pub(crate) fn new() -> Self {
        Self {
            bytes: b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec(),
            // Object zero is free; Catalog and Pages are reserved as 1 and 2.
            offsets: vec![0; 3],
        }
    }

    pub(crate) fn reserve(&mut self) -> usize {
        let id = self.offsets.len();
        self.offsets.push(0);
        id
    }

    fn begin(&mut self, id: usize) {
        self.offsets[id] = self.bytes.len();
        self.bytes
            .extend_from_slice(format!("{id} 0 obj\n").as_bytes());
    }

    pub(crate) fn write(&mut self, id: usize, body: &str) {
        self.begin(id);
        self.bytes.extend_from_slice(body.as_bytes());
        self.bytes.extend_from_slice(b"\nendobj\n");
    }

    pub(crate) fn stream(&mut self, id: usize, entries: &str, data: &[u8]) {
        self.flate_stream(id, entries, &zlib_compress(data, 6));
    }

    /// A stream whose data is zlib-compressed already.
    pub(crate) fn flate_stream(&mut self, id: usize, entries: &str, compressed: &[u8]) {
        self.begin(id);
        self.bytes.extend_from_slice(
            format!(
                "<< /Length {} /Filter /FlateDecode {entries} >>\nstream\n",
                compressed.len()
            )
            .as_bytes(),
        );
        self.bytes.extend_from_slice(compressed);
        self.bytes.extend_from_slice(b"\nendstream\nendobj\n");
    }

    pub(crate) fn finish(mut self, info: usize) -> Vec<u8> {
        // Replacing an outline can leave unused forward page reservations.
        for id in 1..self.offsets.len() {
            if self.offsets[id] == 0 {
                self.write(id, "null");
            }
        }
        let xref = self.bytes.len();
        self.bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", self.offsets.len()).as_bytes(),
        );
        for &offset in &self.offsets[1..] {
            self.bytes
                .extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        self.bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R /Info {info} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                self.offsets.len()
            )
            .as_bytes(),
        );
        self.bytes
    }
}

pub(crate) fn number(value: f64) -> String {
    assert!(
        value.is_finite(),
        "PDF coordinates and metrics must be finite"
    );
    let mut text = format!("{value:.3}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    if text == "-0" {
        text = "0".into();
    }
    text
}

pub(crate) fn utf16_hex(text: &str) -> String {
    let mut hex = String::new();
    for unit in text.encode_utf16() {
        hex.push_str(&format!("{unit:04X}"));
    }
    hex
}

pub(crate) fn string(text: &str) -> String {
    if text.bytes().all(|byte| (b' '..=b'~').contains(&byte)) {
        let mut literal = String::from("(");
        for ch in text.chars() {
            if matches!(ch, '\\' | '(' | ')') {
                literal.push('\\');
            }
            literal.push(ch);
        }
        literal.push(')');
        literal
    } else {
        format!("<FEFF{}>", utf16_hex(text))
    }
}

pub(crate) fn uri(text: &str) -> String {
    let mut ascii = String::new();
    for byte in text.bytes() {
        if (b' '..=b'~').contains(&byte) {
            ascii.push(char::from(byte));
        } else {
            ascii.push_str(&format!("%{byte:02X}"));
        }
    }
    string(&ascii)
}
