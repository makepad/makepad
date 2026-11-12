use std::collections::BTreeMap;

use crate::object::{number, utf16_hex, Objects};
use crate::{FontProgram, Glyph};

struct UsedGlyph {
    advance: u16,
    text: String,
}

pub(crate) struct Font {
    pub(crate) object: usize,
    pub(crate) program: FontProgram,
    used: BTreeMap<u16, UsedGlyph>,
}

impl Font {
    pub(crate) fn new(object: usize, program: FontProgram) -> Self {
        assert!(
            program.units_per_em != 0,
            "font units_per_em must be nonzero"
        );
        Self {
            object,
            program,
            used: BTreeMap::new(),
        }
    }

    pub(crate) fn record(&mut self, glyph: &Glyph) {
        let used = self.used.entry(glyph.id).or_insert_with(|| UsedGlyph {
            advance: glyph.advance,
            text: String::new(),
        });
        if used.text.is_empty() && !glyph.text.is_empty() {
            used.text.clone_from(&glyph.text);
        }
    }

    pub(crate) fn write(self, objects: &mut Objects, index: usize) {
        let file = objects.reserve();
        let descriptor = objects.reserve();
        let descendant = objects.reserve();
        let unicode = objects.reserve();
        let cff = is_cff(&self.program.data);
        let name = base_name(index, &self.program.postscript_name);
        let scale = 1000.0 / f64::from(self.program.units_per_em);
        let metric = |value: i16| number(f64::from(value) * scale);

        objects.stream(
            file,
            &if cff {
                String::from("/Subtype /OpenType")
            } else {
                format!("/Length1 {}", self.program.data.len())
            },
            &self.program.data,
        );
        let bbox = self.program.bbox.map(metric).join(" ");
        objects.write(
            descriptor,
            &format!(
                "<< /Type /FontDescriptor /FontName /{name} /Flags 32 /FontBBox [{bbox}] \
                 /Ascent {} /Descent {} /CapHeight {} /ItalicAngle 0 /StemV 80 /{} {file} 0 R >>",
                metric(self.program.ascent),
                metric(self.program.descent),
                metric(self.program.cap_height),
                if cff { "FontFile3" } else { "FontFile2" },
            ),
        );
        objects.write(
            descendant,
            &format!(
                "<< /Type /Font /Subtype /{} /BaseFont /{name} \
                 /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
                 /FontDescriptor {descriptor} 0 R /W [{}] {} >>",
                if cff { "CIDFontType0" } else { "CIDFontType2" },
                self.widths(scale),
                if cff { "" } else { "/CIDToGIDMap /Identity" },
            ),
        );
        objects.stream(unicode, "", self.cmap().as_bytes());
        objects.write(
            self.object,
            &format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H \
                 /DescendantFonts [{descendant} 0 R] /ToUnicode {unicode} 0 R >>"
            ),
        );
    }

    fn widths(&self, scale: f64) -> String {
        let mut widths = String::new();
        let mut previous: Option<u16> = None;
        for (&id, glyph) in &self.used {
            if previous.and_then(|id| id.checked_add(1)) != Some(id) {
                if previous.is_some() {
                    widths.push_str("] ");
                }
                widths.push_str(&format!("{id} ["));
            } else {
                widths.push(' ');
            }
            widths.push_str(&number(f64::from(glyph.advance) * scale));
            previous = Some(id);
        }
        if previous.is_some() {
            widths.push(']');
        }
        widths
    }

    fn cmap(&self) -> String {
        let mut cmap = String::from(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
             /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
             /CMapName /MakepadToUnicode def\n/CMapType 2 def\n\
             1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
        );
        let mut mappings = self.used.iter().filter(|(_, glyph)| !glyph.text.is_empty());
        loop {
            let mut chunk = String::new();
            let mut count = 0;
            for (&id, glyph) in mappings.by_ref().take(100) {
                chunk.push_str(&format!("<{id:04X}> <{}>\n", utf16_hex(&glyph.text)));
                count += 1;
            }
            if count == 0 {
                break;
            }
            cmap.push_str(&format!("{count} beginbfchar\n{chunk}endbfchar\n"));
        }
        cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        cmap
    }
}

fn is_cff(data: &[u8]) -> bool {
    assert!(data.len() >= 12, "font must contain an sfnt header");
    let count = usize::from(u16::from_be_bytes([data[4], data[5]]));
    let directory = data
        .get(12..12 + count * 16)
        .expect("font must contain its complete sfnt table directory");
    let mut glyf = false;
    for record in directory.chunks_exact(16) {
        if &record[..4] == b"CFF " {
            return true;
        }
        glyf |= &record[..4] == b"glyf";
    }
    assert!(glyf, "font must contain a glyf or CFF table");
    false
}

fn base_name(mut index: usize, name: &str) -> String {
    let mut tag = [b'A'; 6];
    for letter in tag.iter_mut().rev() {
        *letter += (index % 26) as u8;
        index /= 26;
    }
    let mut result: String = tag.into_iter().map(char::from).collect();
    result.push('+');
    if name.is_empty() {
        result.push_str("Font");
    } else {
        for ch in name.chars() {
            result.push(if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            });
        }
    }
    result
}
