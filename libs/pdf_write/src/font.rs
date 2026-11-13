use std::collections::BTreeMap;

use crate::object::{number, utf16_hex, Objects};
use crate::{FontMetrics, Glyph, Type3Glyph};

struct UsedGlyph {
    advance: u16,
    text: String,
    ordinal: usize,
}

struct Program {
    data: Vec<u8>,
    cff: bool,
}

enum Kind {
    Sfnt(Option<Program>),
    Type3(BTreeMap<u16, Type3Glyph>),
}

pub(crate) struct Font {
    pub(crate) objects: Vec<usize>,
    pub(crate) metrics: FontMetrics,
    kind: Kind,
    used: BTreeMap<u16, UsedGlyph>,
    order: Vec<u16>,
}

impl Font {
    pub(crate) fn new(metrics: FontMetrics, type3: bool) -> Self {
        assert!(
            metrics.units_per_em != 0,
            "font units_per_em must be nonzero"
        );
        Self {
            objects: Vec::new(),
            metrics,
            kind: if type3 {
                Kind::Type3(BTreeMap::new())
            } else {
                Kind::Sfnt(None)
            },
            used: BTreeMap::new(),
            order: Vec::new(),
        }
    }

    pub(crate) fn set_program(&mut self, data: Vec<u8>) {
        assert!(
            matches!(self.kind, Kind::Sfnt(_)),
            "set_font_program requires an sfnt font"
        );
        self.kind = Kind::Sfnt(Some(Program {
            cff: is_cff(&data),
            data,
        }));
    }

    pub(crate) fn set_glyph(&mut self, id: u16, glyph: Type3Glyph) {
        assert!(
            matches!(self.kind, Kind::Type3(_)),
            "set_type3_glyph requires a Type 3 font"
        );
        if let Kind::Type3(glyphs) = &mut self.kind {
            glyphs.insert(id, glyph);
        }
    }

    pub(crate) fn used_glyphs(&self) -> Vec<u16> {
        self.used.keys().copied().collect()
    }

    pub(crate) fn conflicts(&self, glyph: &Glyph) -> bool {
        self.used
            .get(&glyph.id)
            .is_some_and(|used| !used.text.is_empty() && used.text != glyph.text)
    }

    pub(crate) fn record(&mut self, glyph: &Glyph, objects: &mut Objects) -> (usize, String) {
        let ordinal = self.order.len();
        let used = self.used.entry(glyph.id).or_insert_with(|| {
            self.order.push(glyph.id);
            UsedGlyph {
                advance: glyph.advance,
                text: String::new(),
                ordinal,
            }
        });
        if used.text.is_empty() && !glyph.text.is_empty() {
            used.text.clone_from(&glyph.text);
        }
        let (subfont, code) = match self.kind {
            Kind::Sfnt(_) => (0, format!("<{:04X}>", glyph.id)),
            Kind::Type3(_) => (used.ordinal / 256, format!("<{:02X}>", used.ordinal % 256)),
        };
        if subfont == self.objects.len() {
            self.objects.push(objects.reserve());
        }
        (subfont, code)
    }

    pub(crate) fn resource_name(&self, index: usize, subfont: usize) -> String {
        match self.kind {
            Kind::Sfnt(_) => format!("F{index}"),
            Kind::Type3(_) => format!("F{index}_{subfont}"),
        }
    }

    pub(crate) fn write(self, objects: &mut Objects, index: usize) {
        if self.used.is_empty() {
            return;
        }
        match &self.kind {
            Kind::Sfnt(program) => {
                assert!(
                    program.is_some(),
                    "font {} has no sfnt program",
                    self.metrics.postscript_name
                );
                self.write_sfnt(objects, index, program.as_ref().unwrap());
            }
            Kind::Type3(glyphs) => self.write_type3(objects, glyphs),
        }
    }

    fn write_sfnt(&self, objects: &mut Objects, index: usize, program: &Program) {
        let file = objects.reserve();
        let descriptor = objects.reserve();
        let descendant = objects.reserve();
        let unicode = objects.reserve();
        let cff = program.cff;
        let name = base_name(index, &self.metrics.postscript_name);
        let scale = 1000.0 / f64::from(self.metrics.units_per_em);
        let metric = |value: i16| number(f64::from(value) * scale);

        objects.stream(
            file,
            &if cff {
                String::from("/Subtype /OpenType")
            } else {
                format!("/Length1 {}", program.data.len())
            },
            &program.data,
        );
        let bbox = self.metrics.bbox.map(metric).join(" ");
        objects.write(
            descriptor,
            &format!(
                "<< /Type /FontDescriptor /FontName /{name} /Flags 32 /FontBBox [{bbox}] \
                 /Ascent {} /Descent {} /CapHeight {} /ItalicAngle 0 /StemV 80 /{} {file} 0 R >>",
                metric(self.metrics.ascent),
                metric(self.metrics.descent),
                metric(self.metrics.cap_height),
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
        objects.stream(
            unicode,
            "",
            cmap(
                self.used
                    .iter()
                    .map(|(&id, glyph)| (id, glyph.text.as_str())),
                false,
            )
            .as_bytes(),
        );
        objects.write(
            self.objects[0],
            &format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H \
                 /DescendantFonts [{descendant} 0 R] /ToUnicode {unicode} 0 R >>"
            ),
        );
    }

    fn write_type3(&self, objects: &mut Objects, glyphs: &BTreeMap<u16, Type3Glyph>) {
        // 1/units_per_em in full: `number`'s millipoints would distort the
        // glyphs, or make the matrix zero.
        let scale = format!("{:.12}", 1.0 / f64::from(self.metrics.units_per_em));
        let scale = scale.trim_end_matches('0').trim_end_matches('.');
        let bbox = self
            .metrics
            .bbox
            .map(|value| number(f64::from(value)))
            .join(" ");
        for (subfont, ids) in self.order.chunks(256).enumerate() {
            let mut procedures = String::new();
            let mut encoding = String::new();
            let mut widths = String::new();
            let mut images = String::new();
            for id in ids {
                assert!(
                    glyphs.contains_key(id),
                    "font {} has no Type 3 glyph {id}",
                    self.metrics.postscript_name
                );
                let used = &self.used[id];
                let procedure =
                    crate::type3::write(objects, &glyphs[id], used.advance, *id, &mut images);
                procedures.push_str(&format!("/g{id} {procedure} 0 R "));
                encoding.push_str(&format!("/g{id} "));
                widths.push_str(&format!("{} ", number(f64::from(used.advance))));
            }
            let unicode = objects.reserve();
            objects.stream(
                unicode,
                "",
                cmap(
                    ids.iter()
                        .enumerate()
                        .map(|(code, id)| (code as u16, self.used[id].text.as_str())),
                    true,
                )
                .as_bytes(),
            );
            objects.write(
                self.objects[subfont],
                &format!(
                    "<< /Type /Font /Subtype /Type3 /FontMatrix [{scale} 0 0 {scale} 0 0] \
                 /FontBBox [{bbox}] /CharProcs << {procedures} >> \
                 /Encoding << /Type /Encoding /Differences [0 {encoding}] >> \
                 /FirstChar 0 /LastChar {} /Widths [{widths}] \
                 /Resources << /XObject << {images} >> >> /ToUnicode {unicode} 0 R >>",
                    ids.len() - 1,
                ),
            );
        }
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
}

fn cmap<'a>(mappings: impl Iterator<Item = (u16, &'a str)>, one_byte: bool) -> String {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /MakepadToUnicode def\n/CMapType 2 def\n\
         1 begincodespacerange\n",
    );
    cmap.push_str(if one_byte {
        "<00> <FF>\n"
    } else {
        "<0000> <FFFF>\n"
    });
    cmap.push_str("endcodespacerange\n");
    let mut mappings = mappings.filter(|(_, text)| !text.is_empty());
    loop {
        let mut chunk = String::new();
        let mut count = 0;
        for (id, text) in mappings.by_ref().take(100) {
            let code = if one_byte {
                format!("{id:02X}")
            } else {
                format!("{id:04X}")
            };
            chunk.push_str(&format!("<{code}> <{}>\n", utf16_hex(text)));
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
