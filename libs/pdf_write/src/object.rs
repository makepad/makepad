use makepad_fast_inflate::zlib_compress;

#[derive(Clone, Copy)]
enum XRefEntry {
    Reserved,
    Pending,
    Uncompressed(usize),
    Compressed { stream: usize, index: usize },
}

pub(crate) struct Objects {
    bytes: Vec<u8>,
    entries: Vec<XRefEntry>,
    pending: Vec<(usize, String)>,
}

impl Objects {
    pub(crate) fn new() -> Self {
        Self {
            bytes: b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec(),
            // Object zero is free; Catalog and Pages are reserved as 1 and 2.
            entries: vec![XRefEntry::Reserved; 3],
            pending: Vec::new(),
        }
    }

    pub(crate) fn reserve(&mut self) -> usize {
        let id = self.entries.len();
        self.entries.push(XRefEntry::Reserved);
        id
    }

    fn begin(&mut self, id: usize) {
        self.entries[id] = XRefEntry::Uncompressed(self.bytes.len());
        self.bytes
            .extend_from_slice(format!("{id} 0 obj\n").as_bytes());
    }

    pub(crate) fn write(&mut self, id: usize, body: &str) {
        self.entries[id] = XRefEntry::Pending;
        self.pending.push((id, body.to_owned()));
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
        for id in 1..self.entries.len() {
            if matches!(self.entries[id], XRefEntry::Reserved) {
                self.write(id, "null");
            }
        }

        let mut pending = std::mem::take(&mut self.pending).into_iter().peekable();
        while pending.peek().is_some() {
            let stream = self.reserve();
            let mut header = String::new();
            let mut bodies = Vec::new();
            let mut count = 0;
            for (index, (id, body)) in pending.by_ref().enumerate() {
                header.push_str(&format!("{id} {} ", bodies.len()));
                bodies.extend_from_slice(body.as_bytes());
                bodies.push(b'\n');
                self.entries[id] = XRefEntry::Compressed { stream, index };
                count = index + 1;
                // Include the header, and always keep a complete object before splitting.
                if header.len() + bodies.len() > 256 * 1024 {
                    break;
                }
            }
            let first = header.len();
            let mut data = header.into_bytes();
            data.extend_from_slice(&bodies);
            self.stream(
                stream,
                &format!("/Type /ObjStm /N {count} /First {first}"),
                &data,
            );
        }

        let xref_id = self.reserve();
        let xref = self.bytes.len();
        // The cross-reference stream includes its own entry and determines /Size.
        self.entries[xref_id] = XRefEntry::Uncompressed(xref);
        let fields: Vec<(u8, usize, usize)> = self
            .entries
            .iter()
            .enumerate()
            .map(|(id, entry)| {
                if id == 0 {
                    return (0, 0, 65535);
                }
                match *entry {
                    XRefEntry::Uncompressed(offset) => (1, offset, 0),
                    XRefEntry::Compressed { stream, index } => (2, stream, index),
                    XRefEntry::Reserved | XRefEntry::Pending => {
                        unreachable!("every reserved object must be written")
                    }
                }
            })
            .collect();
        let width = |value: usize| ((usize::BITS - value.leading_zeros()) as usize).div_ceil(8);
        let w2 = width(fields.iter().map(|entry| entry.1).max().unwrap_or(0)).max(1);
        let w3 = width(fields.iter().map(|entry| entry.2).max().unwrap_or(0)).max(2);
        let mut data = Vec::new();
        for (kind, field2, field3) in fields {
            data.push(kind);
            let field2 = field2.to_be_bytes();
            let field3 = field3.to_be_bytes();
            data.extend_from_slice(&field2[field2.len() - w2..]);
            data.extend_from_slice(&field3[field3.len() - w3..]);
        }
        self.stream(
            xref_id,
            &format!(
                "/Type /XRef /Size {} /W [1 {w2} {w3}] /Root 1 0 R /Info {info} 0 R",
                self.entries.len()
            ),
            &data,
        );
        self.bytes
            .extend_from_slice(format!("startxref\n{xref}\n%%EOF\n").as_bytes());
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
