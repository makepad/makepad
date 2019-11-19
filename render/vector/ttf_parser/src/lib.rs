use font::{Font, Glyph, HorizontalMetrics, Outline, OutlinePoint};
use geometry::{
    AffineTransformation, LinearTransformation, Point, Rectangle, Transform, Vector,
};
use internal_iter::ExtendFromInternalIterator;
use std::{mem, result};

#[derive(Clone, Debug)]
pub struct GlyphsParser<'a> {
    glyphs: Vec<Option<Glyph>>,
    advance_width_count: usize,
    hmtx_table_bytes: &'a [u8],
    index_to_loc_format: IndexToLocFormat,
    loca_table_bytes: &'a [u8],
    glyf_table_bytes: &'a [u8],
}

impl<'a> GlyphsParser<'a> {
    fn new(
        glyphs_count: usize,
        advance_width_count: usize,
        hmtx_table_bytes: &'a [u8],
        index_to_loc_format: IndexToLocFormat,
        loca_table_bytes: &'a [u8],
        glyf_table_bytes: &'a [u8],
    ) -> GlyphsParser<'a> {
        GlyphsParser {
            glyphs: vec![None; glyphs_count],
            advance_width_count,
            hmtx_table_bytes,
            index_to_loc_format,
            loca_table_bytes,
            glyf_table_bytes,
        }
    }

    fn parse_glyphs(mut self) -> Result<Vec<Glyph>> {
        for index in 0..self.glyphs.len() {
            self.get_or_parse_glyph(index)?;
        }
        Ok(self
            .glyphs
            .into_iter()
            .map(|glyph| glyph.unwrap())
            .collect())
    }

    fn get_or_parse_glyph(&mut self, index: usize) -> Result<&Glyph> {
        if !self
            .glyphs
            .get(index)
            .map_or(false, |glyphs| glyphs.is_some())
        {
            self.glyphs.resize(index + 1, None);
            self.glyphs[index] = Some(self.parse_glyph(index)?);
        }
        Ok(self.glyphs[index].as_ref().unwrap())
    }

    fn parse_glyph(&mut self, index: usize) -> Result<Glyph> {
        let start = self.parse_offset(index)?;
        let end = self.parse_offset(index + 1)?;
        let bytes = &self.glyf_table_bytes[start..end];
        let horizontal_metrics = self.parse_horizontal_metrics(index)?;
        Ok(if bytes.is_empty() {
            Glyph {
                horizontal_metrics,
                bounds: Rectangle::default(),
                outline: Outline::new(),
            }
        } else {
            let mut reader = Reader::new(&bytes);
            let contour_count = reader.read_i16()?;
            let bounds = Rectangle::new(
                Point::new(reader.read_i16()? as f32, reader.read_i16()? as f32),
                Point::new(reader.read_i16()? as f32, reader.read_i16()? as f32),
            );
            let bytes = &bytes[10..];
            if contour_count >= 0 {
                Self::parse_simple_glyph(bytes, horizontal_metrics, bounds, contour_count as usize)?
            } else {
                self.parse_composite_glyph(bytes, horizontal_metrics, bounds)?
            }
        })
    }

    fn parse_offset(&self, index: usize) -> Result<usize> {
        let mut reader = Reader::new(&self.loca_table_bytes);
        Ok(match self.index_to_loc_format {
            IndexToLocFormat::Short => {
                reader.skip(index * 2)?;
                reader.read_u16()? as usize * 2
            }
            IndexToLocFormat::Long => {
                reader.skip(index * 4)?;
                reader.read_u32()? as usize
            }
        })
    }

    fn parse_horizontal_metrics(&self, index: usize) -> Result<HorizontalMetrics> {
        let mut reader = Reader::new(self.hmtx_table_bytes);
        if index < self.advance_width_count {
            reader.skip(index * 4)?;
            Ok(HorizontalMetrics {
                advance_width: reader.read_u16()? as f32,
                left_side_bearing: reader.read_i16()? as f32,
            })
        } else {
            reader.skip((self.advance_width_count - 1) * 4)?;
            let advance_width = reader.read_u16()? as f32;
            reader.skip(2)?;
            reader.skip((index - self.advance_width_count) * 2)?;
            Ok(HorizontalMetrics {
                advance_width,
                left_side_bearing: reader.read_i16()? as f32,
            })
        }
    }

    fn parse_simple_glyph(
        bytes: &'a [u8],
        horizontal_metrics: HorizontalMetrics,
        bounds: Rectangle,
        contour_count: usize,
    ) -> Result<Glyph> {
        let mut reader = Reader::new(bytes);
        reader.skip((contour_count - 1) * mem::size_of::<u16>())?;
        let point_count = reader.read_u16()? as usize + 1;
        let instruction_count = reader.read_u16()? as usize;
        reader.skip(instruction_count)?;
        let mut flags_bytes_count = 0;
        let mut x_coordinates_bytes_count = 0;
        let mut y_coordinates_bytes_count = 0;
        let mut flags_count = point_count;
        while flags_count > 0 {
            let flags = SimpleGlyphFlags(reader.read_u8()?);
            let repeat_count = if flags.repeat_flag() {
                flags_bytes_count += 2;
                1 + reader.read_u8()? as usize
            } else {
                flags_bytes_count += 1;
                1
            };
            if repeat_count > flags_count {
                return Err(Error);
            }
            if flags.x_short_vector() {
                x_coordinates_bytes_count += repeat_count;
            } else if !flags.x_is_same_or_positive_x_short_vector() {
                x_coordinates_bytes_count += repeat_count * mem::size_of::<i16>();
            }
            if flags.y_short_vector() {
                y_coordinates_bytes_count += repeat_count;
            } else if !flags.y_is_same_or_positive_y_short_vector() {
                y_coordinates_bytes_count += repeat_count * mem::size_of::<i16>();
            }
            flags_count -= repeat_count;
        }
        let instructions_bytes_start = (contour_count + 1) * mem::size_of::<u16>();
        let flags_bytes_start = instructions_bytes_start + instruction_count;
        let x_coordinates_bytes_start = flags_bytes_start + flags_bytes_count;
        let y_coordinates_bytes_start = x_coordinates_bytes_start + x_coordinates_bytes_count;
        if y_coordinates_bytes_count > bytes.len() - y_coordinates_bytes_start {
            return Err(Error);
        }
        let mut end_pts_for_contours_reader = Reader::new(&bytes[..instructions_bytes_start]);
        let mut point_reader = OutlinePointReader::new(
            &bytes[flags_bytes_start..x_coordinates_bytes_start],
            &bytes[x_coordinates_bytes_start..y_coordinates_bytes_start],
            &bytes[y_coordinates_bytes_start..],
        );
        let mut outline = Outline::new();
        let mut start = 0;
        for _ in 0..contour_count {
            let end = end_pts_for_contours_reader.read_u16()? as usize + 1;
            let mut contour = outline.begin_contour();
            for _ in 0..(end - start) {
                contour.push(point_reader.read_outline_point()?);
            }
            contour.end();
            start = end;
        }
        Ok(Glyph {
            horizontal_metrics,
            bounds,
            outline,
        })
    }

    fn parse_composite_glyph(
        &mut self,
        bytes: &'a [u8],
        mut horizontal_metrics: HorizontalMetrics,
        bounds: Rectangle,
    ) -> Result<Glyph> {
        let mut outline = Outline::new();
        let mut reader = Reader::new(bytes);
        let mut flags = CompositeGlyphFlags(reader.read_u16()?);
        loop {
            let component_glyph = self.parse_glyph(reader.read_u16()? as usize)?;
            if flags.use_my_metrics() {
                horizontal_metrics = component_glyph.horizontal_metrics;
            }
            let (argument_1, argument_2) = if flags.arg_1_and_arg_2_are_words() {
                (reader.read_i16()?, reader.read_i16()?)
            } else {
                (reader.read_i8()? as i16, reader.read_i8()? as i16)
            };
            let xy = if flags.we_have_a_scale() {
                LinearTransformation::uniform_scaling(reader.read_f2dot14()?)
            } else if flags.we_have_an_x_and_y_scale() {
                LinearTransformation::scaling(Vector::new(
                    reader.read_f2dot14()?,
                    reader.read_f2dot14()?,
                ))
            } else if flags.we_have_a_two_by_two() {
                LinearTransformation::new(
                    Vector::new(reader.read_f2dot14()?, reader.read_f2dot14()?),
                    Vector::new(reader.read_f2dot14()?, reader.read_f2dot14()?),
                )
            } else {
                LinearTransformation::identity()
            };
            let z = if flags.args_are_xy_values() {
                Vector::new(
                    xy.x.x.hypot(xy.y.x) * argument_1 as f32,
                    xy.x.y.hypot(xy.y.y) * argument_2 as f32,
                )
            } else {
                component_glyph
                    .outline
                    .points()
                    .get(argument_2 as usize)
                    .ok_or(Error)?
                    .point
                    .transform(&xy)
                    - outline
                        .points()
                        .get(argument_1 as usize)
                        .ok_or(Error)?
                        .point
            };
            for component_contour in component_glyph.outline.contours() {
                let mut contour = outline.begin_contour();
                contour.extend_from_internal_iter(
                    component_contour
                        .points()
                        .iter()
                        .cloned()
                        .map(|point| point.transform(&AffineTransformation::new(xy, z))),
                );
                contour.end();
            }
            if !flags.more_components() {
                break;
            }
            flags = CompositeGlyphFlags(reader.read_u16()?);
        }
        if flags.we_have_instructions() {
            let instruction_length = reader.read_u16()? as usize;
            reader.skip(instruction_length)?;
        }
        Ok(Glyph {
            horizontal_metrics,
            bounds,
            outline,
        })
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq)]
enum IndexToLocFormat {
    Short,
    Long,
}

impl IndexToLocFormat {
    fn from_i16(value: i16) -> Option<IndexToLocFormat> {
        match value {
            0 => Some(IndexToLocFormat::Short),
            1 => Some(IndexToLocFormat::Long),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
struct OutlinePointReader<'a> {
    flags_reader: SimpleGlyphFlagsReader<'a>,
    x_coordinates_reader: Reader<'a>,
    y_coordinates_reader: Reader<'a>,
    current_point: Point,
}

impl<'a> OutlinePointReader<'a> {
    fn new(
        flags_bytes: &'a [u8],
        x_coordinates_bytes: &'a [u8],
        y_coordinates_bytes: &'a [u8],
    ) -> OutlinePointReader<'a> {
        OutlinePointReader {
            flags_reader: SimpleGlyphFlagsReader::new(flags_bytes),
            x_coordinates_reader: Reader::new(x_coordinates_bytes),
            y_coordinates_reader: Reader::new(y_coordinates_bytes),
            current_point: Point::origin(),
        }
    }

    fn read_outline_point(&mut self) -> Result<OutlinePoint> {
        let flags = self.flags_reader.read()?;
        self.current_point += Vector::new(
            if flags.x_short_vector() {
                let x = self.x_coordinates_reader.read_u8()? as f32;
                if flags.x_is_same_or_positive_x_short_vector() {
                    x
                } else {
                    -x
                }
            } else {
                if flags.x_is_same_or_positive_x_short_vector() {
                    0.0
                } else {
                    self.x_coordinates_reader.read_i16()? as f32
                }
            },
            if flags.y_short_vector() {
                let y = self.y_coordinates_reader.read_u8()? as f32;
                if flags.y_is_same_or_positive_y_short_vector() {
                    y
                } else {
                    -y
                }
            } else {
                if flags.y_is_same_or_positive_y_short_vector() {
                    0.0
                } else {
                    self.y_coordinates_reader.read_i16()? as f32
                }
            },
        );
        Ok(OutlinePoint {
            is_on_curve: flags.on_curve_point(),
            point: self.current_point,
        })
    }
}

#[derive(Clone, Debug)]
struct SimpleGlyphFlagsReader<'a> {
    reader: Reader<'a>,
    flags: SimpleGlyphFlags,
    repeat_count: usize,
}

impl<'a> SimpleGlyphFlagsReader<'a> {
    fn new(bytes: &'a [u8]) -> SimpleGlyphFlagsReader<'a> {
        SimpleGlyphFlagsReader {
            reader: Reader::new(bytes),
            flags: SimpleGlyphFlags(0),
            repeat_count: 0,
        }
    }

    fn read(&mut self) -> Result<SimpleGlyphFlags> {
        if self.repeat_count == 0 {
            self.flags = SimpleGlyphFlags(self.reader.read_u8()?);
            self.repeat_count = if self.flags.repeat_flag() {
                self.reader.read_u8()? as usize
            } else {
                0
            };
        } else {
            self.repeat_count -= 1;
        }
        Ok(self.flags)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct SimpleGlyphFlags(u8);

impl SimpleGlyphFlags {
    fn on_curve_point(self) -> bool {
        self.0 & (1 << 0) != 0
    }

    fn x_short_vector(self) -> bool {
        self.0 & (1 << 1) != 0
    }

    fn y_short_vector(self) -> bool {
        self.0 & (1 << 2) != 0
    }

    fn repeat_flag(self) -> bool {
        self.0 & (1 << 3) != 0
    }

    fn x_is_same_or_positive_x_short_vector(self) -> bool {
        self.0 & (1 << 4) != 0
    }

    fn y_is_same_or_positive_y_short_vector(self) -> bool {
        self.0 & (1 << 5) != 0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct CompositeGlyphFlags(u16);

impl CompositeGlyphFlags {
    fn arg_1_and_arg_2_are_words(self) -> bool {
        self.0 & (1 << 0) != 0
    }

    fn args_are_xy_values(self) -> bool {
        self.0 & (1 << 1) != 0
    }

    fn we_have_a_scale(self) -> bool {
        self.0 & (1 << 3) != 0
    }

    fn more_components(self) -> bool {
        self.0 & (1 << 5) != 0
    }

    fn we_have_an_x_and_y_scale(self) -> bool {
        self.0 & (1 << 6) != 0
    }

    fn we_have_a_two_by_two(self) -> bool {
        self.0 & (1 << 7) != 0
    }

    fn we_have_instructions(self) -> bool {
        self.0 & (1 << 8) != 0
    }

    fn use_my_metrics(self) -> bool {
        self.0 & (1 << 9) != 0
    }
}

#[derive(Clone, Debug)]
struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes }
    }

    fn skip(&mut self, count: usize) -> Result<()> {
        if count > self.bytes.len() {
            return Err(Error);
        }
        self.bytes = &self.bytes[count..];
        Ok(())
    }

    fn read(&mut self, bytes: &mut [u8]) -> Result<()> {
        if bytes.len() > self.bytes.len() {
            return Err(Error);
        }
        bytes.copy_from_slice(&self.bytes[..bytes.len()]);
        self.bytes = &self.bytes[bytes.len()..];
        Ok(())
    }

    fn read_i8(&mut self) -> Result<i8> {
        let mut bytes = [0; mem::size_of::<i8>()];
        self.read(&mut bytes)?;
        Ok(i8::from_be_bytes(bytes))
    }

    fn read_i16(&mut self) -> Result<i16> {
        let mut bytes = [0; mem::size_of::<i16>()];
        self.read(&mut bytes)?;
        Ok(i16::from_be_bytes(bytes))
    }

    fn read_u8(&mut self) -> Result<u8> {
        let mut bytes = [0; mem::size_of::<u8>()];
        self.read(&mut bytes)?;
        Ok(u8::from_be_bytes(bytes))
    }

    fn read_u16(&mut self) -> Result<u16> {
        let mut bytes = [0; mem::size_of::<u16>()];
        self.read(&mut bytes)?;
        Ok(u16::from_be_bytes(bytes))
    }

    fn read_u32(&mut self) -> Result<u32> {
        let mut bytes = [0; mem::size_of::<u32>()];
        self.read(&mut bytes)?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn read_f2dot14(&mut self) -> Result<f32> {
        Ok(self.read_i16()? as f32 / (1 << 14) as f32)
    }
}

pub type Result<T> = result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Error;

pub fn parse_ttf(bytes: &[u8]) -> Result<Font> {
    let mut reader = Reader::new(&bytes[0..12]);
    let sfnt_version = reader.read_u32()?;
    if ![0x00010000, u32::from_be_bytes(*b"true")].contains(&sfnt_version) {
        return Err(Error);
    }
    let table_count = reader.read_u16()? as usize;
    reader.skip(6)?;
    let mut cmap_table_bytes = None;
    let mut glyf_table_bytes = None;
    let mut head_table_bytes = None;
    let mut hhea_table_bytes = None;
    let mut hmtx_table_bytes = None;
    let mut loca_table_bytes = None;
    let mut maxp_table_bytes = None;
    for index in 0..table_count {
        let mut reader = Reader::new(&bytes[(12 + index * 16)..][..16]);
        let table_tag = reader.read_u32()?;
        reader.skip(4)?;
        let offset = reader.read_u32()? as usize;
        let length = reader.read_u32()? as usize;
        let table_bytes = &bytes[offset..][..length];
        match &table_tag.to_be_bytes() {
            b"cmap" => cmap_table_bytes = Some(table_bytes),
            b"glyf" => glyf_table_bytes = Some(table_bytes),
            b"head" => head_table_bytes = Some(table_bytes),
            b"hhea" => hhea_table_bytes = Some(table_bytes),
            b"hmtx" => hmtx_table_bytes = Some(table_bytes),
            b"loca" => loca_table_bytes = Some(table_bytes),
            b"maxp" => maxp_table_bytes = Some(table_bytes),
            _ => {}
        }
    }
    let cmap_table_bytes = cmap_table_bytes.ok_or(Error)?;
    let glyf_table_bytes = glyf_table_bytes.ok_or(Error)?;
    let head_table_bytes = head_table_bytes.ok_or(Error)?;
    let hhea_table_bytes = hhea_table_bytes.ok_or(Error)?;
    let hmtx_table_bytes = hmtx_table_bytes.ok_or(Error)?;
    let loca_table_bytes = loca_table_bytes.ok_or(Error)?;
    let maxp_table_bytes = maxp_table_bytes.ok_or(Error)?;
    let mut reader = Reader::new(hhea_table_bytes);
    reader.skip(4)?;
    let ascender = reader.read_i16()? as f32;
    let descender = reader.read_i16()? as f32;
    let line_gap = reader.read_i16()? as f32;
    reader.skip(24)?;
    let advance_width_count = reader.read_u16()? as usize;
    let mut reader = Reader::new(maxp_table_bytes);
    reader.skip(4)?;
    let glyph_count = reader.read_u16()? as usize;
    reader.skip(26)?;
    let mut reader = Reader::new(head_table_bytes);
    reader.skip(18)?;
    let units_per_em = reader.read_u16()? as f32;
    reader.skip(16)?;
    let bounds = Rectangle::new(
        Point::new(reader.read_i16()? as f32, reader.read_i16()? as f32),
        Point::new(reader.read_i16()? as f32, reader.read_i16()? as f32),
    );
    reader.skip(6)?;
    let index_to_loc_format = IndexToLocFormat::from_i16(reader.read_i16()?).ok_or(Error)?;
    reader.skip(2)?;
    Ok(Font {
        units_per_em,
        ascender,
        descender,
        line_gap,
        bounds,
        char_code_to_glyph_index_map: parse_char_code_to_glyph_index_map(cmap_table_bytes)?,
        glyphs: GlyphsParser::new(
            glyph_count,
            advance_width_count,
            hmtx_table_bytes,
            index_to_loc_format,
            loca_table_bytes,
            glyf_table_bytes,
        )
        .parse_glyphs()?,
    })
}

fn parse_char_code_to_glyph_index_map(bytes: &[u8]) -> Result<Vec<usize>> {
    let mut reader = Reader::new(bytes);
    reader.skip(2)?;
    let mut subtable_bytes = None;
    let subtable_count = reader.read_u16()? as usize;
    for _ in 0..subtable_count {
        let platform_id = reader.read_u16()?;
        let encoding_id = reader.read_u16()?;
        let offset = reader.read_u32()? as usize;
        if let (0, _) | (3, 1) | (3, 10) = (platform_id, encoding_id) {
            subtable_bytes = Some(&bytes[offset..]);
            break;
        }
    }
    let subtable_bytes = subtable_bytes.ok_or(Error)?;
    let mut reader = Reader::new(subtable_bytes);
    let format = reader.read_u16()?;
    let bytes = &subtable_bytes[2..];
    match format {
        4 => parse_char_code_to_glyph_index_map_format_4(bytes),
        _ => Err(Error),
    }
}

fn parse_char_code_to_glyph_index_map_format_4(bytes: &[u8]) -> Result<Vec<usize>> {
    let mut reader = Reader::new(bytes);
    reader.skip(4)?;
    let seg_count = reader.read_u16()? as usize / 2;
    let end_code_bytes_start = 12;
    let end_code_bytes_end = end_code_bytes_start + seg_count * 2;
    let start_code_bytes_start = end_code_bytes_end + 2;
    let id_delta_bytes_start = start_code_bytes_start + seg_count * 2;
    let id_range_offset_bytes_start = id_delta_bytes_start + seg_count * 2;
    let end_code_bytes = &bytes[end_code_bytes_start..end_code_bytes_end];
    let start_code_bytes = &bytes[start_code_bytes_start..id_delta_bytes_start];
    let id_delta_bytes = &bytes[id_delta_bytes_start..id_range_offset_bytes_start];
    let id_range_offset_bytes = &bytes[id_range_offset_bytes_start..];
    let mut end_code_reader = Reader::new(end_code_bytes);
    let mut start_code_reader = Reader::new(start_code_bytes);
    let mut id_delta_reader = Reader::new(id_delta_bytes);
    let mut id_range_offset_reader = Reader::new(id_range_offset_bytes);
    let mut char_code_to_glyph_index_map = Vec::new();
    for seg_index in 0..seg_count {
        let end_code = end_code_reader.read_u16()?;
        let start_code = start_code_reader.read_u16()?;
        let id_delta = id_delta_reader.read_u16()? as usize;
        let id_range_offset = id_range_offset_reader.read_u16()? as usize;
        for code in start_code..end_code {
            let mut id = if id_range_offset == 0 {
                code
            } else {
                let id_range_bytes = &id_range_offset_bytes[(seg_index * 2)..];
                let mut reader = Reader::new(id_range_bytes);
                reader.skip(id_range_offset + (code - start_code) as usize * 2)?;
                let id = reader.read_u16()?;
                id
            } as usize;
            if id != 0 {
                id = (id + id_delta) % 65536;
            }
            char_code_to_glyph_index_map.resize(code as usize + 1, 0);
            char_code_to_glyph_index_map[code as usize] = id;
        }
    }
    Ok(char_code_to_glyph_index_map)
}
