use super::{
    font::{Font, GlyphId},
    font_atlas::{ColorAtlas, GlyphImage, GlyphImageKey, GrayscaleAtlas},
    geom::{Point, Rect, Size},
    image::Image,
    sdfer,
    sdfer::Sdfer,
};

#[derive(Debug)]
pub struct Rasterizer {
    sdfer: Sdfer,
    grayscale_atlas: GrayscaleAtlas,
    color_atlas: ColorAtlas,
}

impl Rasterizer {
    pub fn new(settings: Settings) -> Self {
        Self {
            sdfer: Sdfer::new(settings.sdfer),
            grayscale_atlas: GrayscaleAtlas::new(settings.grayscale_atlas_size),
            color_atlas: ColorAtlas::new(settings.color_atlas_size),
        }
    }

    pub fn sdfer(&self) -> &Sdfer {
        &self.sdfer
    }

    pub fn grayscale_atlas(&self) -> &GrayscaleAtlas {
        &self.grayscale_atlas
    }

    pub fn color_atlas(&self) -> &ColorAtlas {
        &self.color_atlas
    }

    pub fn grayscale_atlas_mut(&mut self) -> &mut GrayscaleAtlas {
        &mut self.grayscale_atlas
    }

    pub fn color_atlas_mut(&mut self) -> &mut ColorAtlas {
        &mut self.color_atlas
    }

    pub fn rasterize_glyph(
        &mut self,
        font: &Font,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        if let Some(rasterized_glyph) = self.rasterize_glyph_outline(font, glyph_id, dpxs_per_em) {
            return Some(rasterized_glyph);
        };
        if let Some(rasterized_glyph) =
            self.rasterize_glyph_raster_image(font, glyph_id, dpxs_per_em)
        {
            return Some(rasterized_glyph);
        }
        None
    }

    fn rasterize_glyph_outline(
        &mut self,
        font: &Font,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        let dpxs_per_em = if dpxs_per_em < 32.0 { 32.0 } else { 64.0 };
        let dpxs_per_em = dpxs_per_em * 2.0;
        let mut outline = None;
        let bounds_in_ems = font.glyph_outline_bounds_in_ems(glyph_id, &mut outline)?;
        let atlas_image_size = glyph_outline_image_size(bounds_in_ems.size, dpxs_per_em);
        let atlas_image_padding = self.sdfer.settings().padding;
        let atlas_image_bounds =
            match self
                .grayscale_atlas
                .get_or_allocate_glyph_image(GlyphImageKey {
                    font_id: font.id(),
                    glyph_id,
                    size: atlas_image_size + Size::from(self.sdfer.settings().padding) * 2,
                })? {
                GlyphImage::Cached(rect) => rect,
                GlyphImage::Allocated(mut sdf) => {
                    let outline = outline.unwrap_or_else(|| font.glyph_outline(glyph_id).unwrap());
                    let mut coverage = Image::new(atlas_image_size);
                    outline.rasterize(
                        dpxs_per_em,
                        &mut coverage.subimage_mut(atlas_image_size.into()),
                    );
                    self.sdfer
                        .coverage_to_sdf(&coverage.subimage(atlas_image_size.into()), &mut sdf);
                    sdf.bounds()
                }
            };

        return Some(RasterizedGlyph {
            atlas_kind: AtlasKind::Grayscale,
            atlas_size: self.grayscale_atlas().size(),
            atlas_image_bounds,
            atlas_image_padding,
            origin_in_dpxs: bounds_in_ems.origin * dpxs_per_em,
            dpxs_per_em,
        });
    }

    fn rasterize_glyph_raster_image(
        &mut self,
        font: &Font,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        const PADDING: usize = 2;

        let raster_image = font.glyph_raster_image(glyph_id, dpxs_per_em)?;
        let atlas_image_bounds =
            match self
                .color_atlas
                .get_or_allocate_glyph_image(GlyphImageKey {
                    font_id: font.id(),
                    glyph_id,
                    size: raster_image.decode_size() + Size::from(2 * PADDING),
                })? {
                GlyphImage::Cached(rect) => rect,
                GlyphImage::Allocated(mut image) => {
                    let size = image.size();
                    image = image.subimage_mut(Rect::from(size).unpad(PADDING));
                    raster_image.decode(&mut image);
                    image.bounds()
                }
            };
        return Some(RasterizedGlyph {
            atlas_kind: AtlasKind::Color,
            atlas_size: self.color_atlas.size(),
            atlas_image_bounds,
            atlas_image_padding: PADDING,
            origin_in_dpxs: raster_image.origin_in_dpxs(),
            dpxs_per_em: raster_image.dpxs_per_em(),
        });
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub sdfer: sdfer::Settings,
    pub grayscale_atlas_size: Size<usize>,
    pub color_atlas_size: Size<usize>,
}

#[derive(Clone, Copy, Debug)]
pub struct RasterizedGlyph {
    pub atlas_kind: AtlasKind,
    pub atlas_size: Size<usize>,
    pub atlas_image_bounds: Rect<usize>,
    pub atlas_image_padding: usize,
    pub origin_in_dpxs: Point<f32>,
    pub dpxs_per_em: f32,
}

#[derive(Clone, Copy, Debug)]
pub enum AtlasKind {
    Grayscale,
    Color,
}

fn glyph_outline_image_size(size_in_ems: Size<f32>, dpxs_per_em: f32) -> Size<usize> {
    let size_in_dpxs = size_in_ems * dpxs_per_em;
    Size::new(
        size_in_dpxs.width.ceil() as usize,
        size_in_dpxs.height.ceil() as usize,
    )
}

/*
use {
    crate::{
        font_atlas::CxFontAtlas,
        font_loader::{FontId, FontLoader},
        sdf_glyph_rasterizer::SdfGlyphRasterizer,
        svg_glyph_rasterizer::SvgGlyphRasterizer,
    },
    makepad_platform::*,
    std::{
        collections::HashMap,
        fs::{File, OpenOptions},
        io::{self, Read, Write},
        path::Path,
    },
};

#[derive(Debug)]
pub struct GlyphRasterizer {
    sdf_glyph_rasterizer: SdfGlyphRasterizer,
    svg_glyph_rasterizer: SvgGlyphRasterizer,
    cache: Cache,
}

impl GlyphRasterizer {
    pub fn new(cache_dir: Option<&Path>) -> Self {
        Self {
            sdf_glyph_rasterizer: SdfGlyphRasterizer::new(),
            svg_glyph_rasterizer: SvgGlyphRasterizer::new(),
            cache: Cache::new(cache_dir).expect("failed to load glyph raster cache"),
        }
    }

    pub fn get_or_rasterize_glyph(
        &mut self,
        font_loader: &mut FontLoader,
        font_atlas: &mut CxFontAtlas,
        Command {
            mode,
            params:
                params @ Params {
                    font_id,
                    atlas_page_id,
                    glyph_id,
                },
            ..
        }: Command,
    ) -> RasterizedGlyph<'_> {
        let font = font_loader[font_id].as_mut().unwrap();
        let atlas_page = &font.atlas_pages[atlas_page_id];
        let font_size = atlas_page.font_size_in_device_pixels;
        let font_path = font_loader.path(font_id).unwrap();
        let key = CacheKey::new(&font_path, glyph_id, font_size);
        if !self.cache.contains_key(&key) {
            self.cache
                .insert_with(key, |output| match mode {
                    Mode::Sdf => self.sdf_glyph_rasterizer.rasterize_sdf_glyph(
                        font_loader,
                        font_atlas,
                        params,
                        output,
                    ),
                    Mode::Svg => self.svg_glyph_rasterizer.rasterize_svg_glyph(
                        font_loader,
                        font_atlas,
                        params,
                        output,
                    ),
                })
                .expect("failed to update glyph raster cache")
        }
        self.cache.get(key).unwrap()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Command {
    pub mode: Mode,
    pub params: Params,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Mode {
    Svg,
    Sdf,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Params {
    pub font_id: FontId,
    pub atlas_page_id: usize,
    pub glyph_id: usize,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RasterizedGlyph<'a> {
    pub size: SizeUsize,
    pub bytes: &'a [u8],
}

#[derive(Debug)]
struct Cache {
    data: Vec<u8>,
    data_file: Option<File>,
    index: HashMap<CacheKey, CacheIndexEntry>,
    index_file: Option<File>,
}

impl Cache {
    fn new(dir: Option<&Path>) -> io::Result<Self> {
        let mut data_file = match dir {
            Some(dir) => Some(
                OpenOptions::new()
                    .create(true)
                    .read(true)
                    .write(true)
                    .open(dir.join("glyph_raster_data"))?,
            ),
            None => None,
        };

        let mut data = Vec::new();
        if let Some(data_file) = &mut data_file {
            data_file.read_to_end(&mut data)?;
        }

        let mut index_file = match dir {
            Some(dir) => Some(
                OpenOptions::new()
                    .create(true)
                    .read(true)
                    .write(true)
                    .open(dir.join("glyph_raster_index"))?,
            ),
            None => None,
        };

        let mut index = HashMap::new();
        if let Some(index_file) = &mut index_file {
            loop {
                let mut buffer = [0; 32];
                match index_file.read_exact(&mut buffer) {
                    Ok(_) => (),
                    Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
                    Err(error) => return Err(error),
                }
                index.insert(
                    CacheKey::from_bytes(buffer[0..8].try_into().unwrap()),
                    CacheIndexEntry::from_bytes(buffer[8..32].try_into().unwrap()),
                );
            }
        }
        Ok(Self {
            data,
            data_file,
            index,
            index_file,
        })
    }

    fn contains_key(&self, key: &CacheKey) -> bool {
        self.index.contains_key(key)
    }

    fn get(&self, key: CacheKey) -> Option<RasterizedGlyph<'_>> {
        let CacheIndexEntry { size, offset, len } = self.index.get(&key).copied()?;
        Some(RasterizedGlyph {
            size,
            bytes: &self.data[offset..][..len],
        })
    }

    fn insert_with(
        &mut self,
        key: CacheKey,
        f: impl FnOnce(&mut Vec<u8>) -> SizeUsize,
    ) -> io::Result<()> {
        let offset = self.data.len();
        let size = f(&mut self.data);
        let len = self.data.len() - offset;
        if let Some(data_file) = &mut self.data_file {
            data_file.write_all(&self.data[offset..][..len])?;
        }
        let index_entry = CacheIndexEntry { size, offset, len };
        self.index.insert(key, index_entry);
        if let Some(index_file) = &mut self.index_file {
            let mut buffer = [0; 32];
            buffer[0..8].copy_from_slice(&key.to_bytes());
            buffer[8..32].copy_from_slice(&index_entry.to_bytes());
            index_file.write_all(&buffer)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct CacheKey(LiveId);

impl CacheKey {
    fn new(font_path: &str, glyph_id: usize, font_size: f64) -> Self {
        Self(
            LiveId::empty()
                .bytes_append(font_path.as_bytes())
                .bytes_append(&glyph_id.to_ne_bytes())
                .bytes_append(&font_size.to_ne_bytes()),
        )
    }

    fn from_bytes(bytes: [u8; 8]) -> Self {
        Self(LiveId(u64::from_be_bytes(bytes)))
    }

    fn to_bytes(self) -> [u8; 8] {
        self.0 .0.to_be_bytes()
    }
}

#[derive(Clone, Copy, Debug)]
struct CacheIndexEntry {
    size: SizeUsize,
    offset: usize,
    len: usize,
}

impl CacheIndexEntry {
    fn from_bytes(bytes: [u8; 24]) -> Self {
        Self {
            size: SizeUsize {
                width: u32::from_be_bytes(bytes[0..4].try_into().unwrap())
                    .try_into()
                    .unwrap(),
                height: u32::from_be_bytes(bytes[4..8].try_into().unwrap())
                    .try_into()
                    .unwrap(),
            },
            offset: u64::from_be_bytes(bytes[8..16].try_into().unwrap())
                .try_into()
                .unwrap(),
            len: u64::from_be_bytes(bytes[16..24].try_into().unwrap())
                .try_into()
                .unwrap(),
        }
    }

    fn to_bytes(self) -> [u8; 24] {
        let mut bytes = [0; 24];
        bytes[0..4].copy_from_slice(&u32::try_from(self.size.width).unwrap().to_be_bytes());
        bytes[4..8].copy_from_slice(&u32::try_from(self.size.height).unwrap().to_be_bytes());
        bytes[8..16].copy_from_slice(&u64::try_from(self.offset).unwrap().to_be_bytes());
        bytes[16..24].copy_from_slice(&u64::try_from(self.len).unwrap().to_be_bytes());
        bytes
    }
}
*/
