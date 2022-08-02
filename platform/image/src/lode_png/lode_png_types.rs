
use crate::lode_png::lode_png::*;
use std::fmt;
use std::cmp;

pub trait VecExt<T: Clone>{
    fn try_with_capacity(capacity:usize)->Result<Vec<T>,PNGError>;
    fn try_reserve(&mut self, capacity:usize)->Result<(),PNGError>;
    fn try_extend_from_slice(&mut self, slice:&[T])->Result<(),PNGError>;
}

impl<T: Clone> VecExt<T> for Vec<T>{
    fn try_reserve(&mut self, capacity:usize)->Result<(),PNGError>{
        Ok(self.reserve(capacity))
    }
    fn try_with_capacity(capacity:usize)->Result<Vec<T>,PNGError>{
        Ok(Self::with_capacity(capacity))
    }
    fn try_extend_from_slice(&mut self, other:&[T])->Result<(),PNGError>{
        Ok(self.extend_from_slice(other))
    }
}

#[derive(Copy, Debug, Clone, Default, PartialEq)]
#[repr(packed)]
pub struct RGBA{
    pub r:u8,
    pub g:u8,
    pub b:u8,
    pub a:u8,
}
#[derive(Copy, Clone)]

// this really should be an enum but i'm not going to come up with all the names. 
pub struct PNGError(u32);

impl fmt::Debug for PNGError {
    #[cold]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Png Error - {}", error_description(self.0))
    }
}

fn error_description(code:u32) -> &'static str {
    match code {
        0 => "no error, everything went ok\0",
        1 => "nothing done yet\0",

        /*the Encoder/Decoder has done nothing yet, error checking makes no sense yet*/
        10 => "end of input memory reached without huffman end code\0",

        /*while huffman decoding*/
        11 => "error in code tree made it jump outside of huffman tree\0",

        /*while huffman decoding*/
        13 | 14 | 15 => "problem while processing dynamic deflate block\0",
        16 => "unexisting code while processing dynamic deflate block\0",
        18 => "invalid distance code while inflating\0",
        17 | 19 | 22 => "end of out buffer memory reached while inflating\0",
        20 => "invalid deflate block BTYPE encountered while decoding\0",
        21 => "NLEN is not ones complement of LEN in a deflate block\0",

        /*end of out buffer memory reached while inflating:
            This can happen if the inflated deflate data is longer than the amount of bytes required to fill up
            all the pixels of the image, given the color depth and image dimensions. Something that doesn't
            happen in a normal, well encoded, PNG image.*/
        23 => "end of in buffer memory reached while inflating\0",
        24 => "invalid FCHECK in zlib header\0",
        25 => "invalid compression method in zlib header\0",
        26 => "FDICT encountered in zlib header while it\'s not used for PNG\0",
        27 => "PNG file is smaller than a PNG header\0",
        /*Checks the magic file header, the first 8 bytes of the PNG file*/
        28 => "incorrect PNG signature, it\'s no PNG or corrupted\0",
        29 => "first chunk is not the header chunk\0",
        30 => "chunk length too large, chunk broken off at end of file\0",
        31 => "illegal PNG color type or bpp\0",
        32 => "illegal PNG compression method\0",
        33 => "illegal PNG filter method\0",
        34 => "illegal PNG interlace method\0",
        35 => "chunk length of a chunk is too large or the chunk too small\0",
        36 => "illegal PNG filter type encountered\0",
        37 => "illegal bit depth for this color type given\0",
        38 => "the palette is too big\0",
        /*more than 256 colors*/
        39 => "more palette alpha values given in tRNS chunk than there are colors in the palette\0",
        40 => "tRNS chunk has wrong size for greyscale image\0",
        41 => "tRNS chunk has wrong size for RGB image\0",
        42 => "tRNS chunk appeared while it was not allowed for this color type\0",
        43 => "bKGD chunk has wrong size for palette image\0",
        44 => "bKGD chunk has wrong size for greyscale image\0",
        45 => "bKGD chunk has wrong size for RGB image\0",
        48 => "empty input buffer given to decoder. Maybe caused by non-existing file?\0",
        49 | 50 => "jumped past memory while generating dynamic huffman tree\0",
        51 => "jumped past memory while inflating huffman block\0",
        52 => "jumped past memory while inflating\0",
        53 => "size of zlib data too small\0",
        54 => "repeat symbol in tree while there was no value symbol yet\0",

        /*jumped past tree while generating huffman tree, this could be when the
           tree will have more leaves than symbols after generating it out of the
           given lenghts. They call this an oversubscribed dynamic bit lengths tree in zlib.*/
        55 => "jumped past tree while generating huffman tree\0",
        56 => "given output image colortype or bitdepth not supported for color conversion\0",
        57 => "invalid CRC encountered (checking CRC can be disabled)\0",
        58 => "invalid ADLER32 encountered (checking ADLER32 can be disabled)\0",
        59 => "requested color conversion not supported\0",
        60 => "invalid window size given in the settings of the encoder (must be 0-32768)\0",
        61 => "invalid BTYPE given in the settings of the encoder (only 0, 1 and 2 are allowed)\0",

        /*LodePNG leaves the choice of RGB to greyscale conversion formula to the user.*/
        62 => "conversion from color to greyscale not supported\0",
        63 => "length of a chunk too long, max allowed for PNG is 2147483647 bytes per chunk\0",

        /*(2^31-1)*/
        /*this would result in the inability of a deflated block to ever contain an end code. It must be at least 1.*/
        64 => "the length of the END symbol 256 in the Huffman tree is 0\0",
        66 => "the length of a text chunk keyword given to the encoder is longer than the maximum of 79 bytes\0",
        67 => "the length of a text chunk keyword given to the encoder is smaller than the minimum of 1 byte\0",
        68 => "tried to encode a PLTE chunk with a palette that has less than 1 or more than 256 colors\0",
        69 => "unknown chunk type with \'critical\' flag encountered by the decoder\0",
        71 => "unexisting interlace mode given to encoder (must be 0 or 1)\0",
        72 => "while decoding, unexisting compression method encountering in zTXt or iTXt chunk (it must be 0)\0",
        73 => "invalid tIME chunk size\0",
        74 => "invalid pHYs chunk size\0",
        /*length could be wrong, or data chopped off*/
        75 => "no null termination char found while decoding text chunk\0",
        76 => "iTXt chunk too short to contain required bytes\0",
        77 => "integer overflow in buffer size\0",
        78 => "failed to open file for reading\0",

        /*file doesn't exist or couldn't be opened for reading*/
        79 => "failed to open file for writing\0",
        80 => "tried creating a tree of 0 symbols\0",
        81 => "lazy matching at pos 0 is impossible\0",
        82 => "color conversion to palette requested while a color isn\'t in palette\0",
        83 => "memory allocation failed\0",
        84 => "given image too small to contain all pixels to be encoded\0",
        86 => "impossible offset in lz77 encoding (internal bug)\0",
        87 => "must provide custom zlib function pointer if LODEPNG_COMPILE_ZLIB is not defined\0",
        88 => "invalid filter strategy given for EncoderSettings.filter_strategy\0",
        89 => "text chunk keyword too short or long: must have size 1-79\0",

        /*the windowsize in the CompressSettings. Requiring POT(==> & instead of %) makes encoding 12% faster.*/
        90 => "windowsize must be a power of two\0",
        91 => "invalid decompressed idat size\0",
        92 => "too many pixels, not supported\0",
        93 => "zero width or height is invalid\0",
        94 => "header chunk must have a size of 13 bytes\0",

        95 => "Out of memory",
        96 => "Zlib error",
        _ => "unknown error code\0",
    }
}

impl PNGError{
    pub fn new(code: u32) -> Self {
        Self(code)
    }    
}

/// automatically use color type with less bits per pixel if losslessly possible. Default: `AUTO`
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FilterStrategy {
    /// every filter at zero
    ZERO = 0,
    /// Use filter that gives minumum sum, as described in the official PNG filter heuristic.
    MINSUM,
    /// Use the filter type that gives smallest Shannon entropy for this scanline. Depending
    /// on the image, this is better or worse than minsum.
    ENTROPY,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ColorType {
    /// greyscale: 1, 2, 4, 8, 16 bit
    GREY = 0,
    /// RGB: 8, 16 bit
    RGB = 2,
    /// palette: 1, 2, 4, 8 bit
    PALETTE = 3,
    /// greyscale with alpha: 8, 16 bit
    #[allow(non_camel_case_types)]
    GREY_ALPHA = 4,
    /// RGB with alpha: 8, 16 bit
    RGBA = 6,

    /// Not PNG standard, for internal use only. BGRA with alpha, 8 bit
    BGRA = 6 | 64,
    /// Not PNG standard, for internal use only. BGR no alpha, 8 bit
    BGR = 2 | 64,
    /// Not PNG standard, for internal use only. BGR no alpha, padded, 8 bit
    BGRX = 3 | 64,
}

impl Default for ColorType{
    fn default()->Self{Self::RGB}
}

impl Default for ColorProfile{
    fn default()->Self{
        Self {
            colored: false,
            key: false,
            key_r: 0,
            key_g: 0,
            key_b: 0,
            alpha: false,
            numcolors: 0,
            bits: 1,
            palette: [RGBA{r:0,g:0,b:0,a:0}; 256],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ColorProfile {
    /// not greyscale
    pub colored: bool,
    /// image is not opaque and color key is possible instead of full alpha
    pub key: bool,
    /// key values, always as 16-bit, in 8-bit case the byte is duplicated, e.g. 65535 means 255
    pub key_r: u16,
    pub key_g: u16,
    pub key_b: u16,
    /// image is not opaque and alpha channel or alpha palette required
    pub alpha: bool,
    /// amount of colors, up to 257. Not valid if bits == 16.
    /// bits per channel (not for palette). 1,2 or 4 for greyscale only. 16 if 16-bit per channel required.
    pub bits: u8,
    pub numcolors: u16,
    /// Remembers up to the first 256 RGBA colors, in no particular order
    pub palette: [RGBA; 256],
}

/// Information about the PNG image, except pixels, width and height
#[derive(Debug, Clone, Default)]
pub struct Info {
    /// interlace method of the original file
    pub interlace_method: u8,
    /// color type and bits, palette and transparency of the PNG file
    pub color: ColorMode,

    ///  suggested background color chunk (bKGD)
    ///  This color uses the same color mode as the PNG (except alpha channel), which can be 1-bit to 16-bit.
    ///
    ///  For greyscale PNGs, r, g and b will all 3 be set to the same. When encoding
    ///  the encoder writes the red one. For palette PNGs: When decoding, the RGB value
    ///  will be stored, not a palette index. But when encoding, specify the index of
    ///  the palette in background_r, the other two are then ignored.
    ///
    ///  The decoder does not use this background color to edit the color of pixels.
    pub background_defined: bool,
    /// red component of suggested background color
    pub background_r: u16,
    /// green component of suggested background color
    pub background_g: u16,
    /// blue component of suggested background color
    pub background_b: u16,

    /// set to 1 to make the encoder generate a tIME chunk
    pub time_defined: bool,
    /// time chunk (tIME)
    pub time: Time,

    /// if 0, there is no pHYs chunk and the values below are undefined, if 1 else there is one
    pub phys_defined: bool,
    /// pixels per unit in x direction
    pub phys_x: u32,
    /// pixels per unit in y direction
    pub phys_y: u32,
    /// may be 0 (unknown unit) or 1 (metre)
    pub phys_unit: u8,

    /// There are 3 buffers, one for each position in the PNG where unknown chunks can appear
    /// each buffer contains all unknown chunks for that position consecutively
    /// The 3 buffers are the unknown chunks between certain critical chunks:
    /// 0: IHDR-`PLTE`, 1: `PLTE`-IDAT, 2: IDAT-IEND
    /// Must be boxed for FFI hack.
    pub(crate) unknown_chunks: [Box<Vec<u8>>; 3],

    ///  non-international text chunks (tEXt and zTXt)
    ///
    ///  The `char**` arrays each contain num strings. The actual messages are in
    ///  text_strings, while text_keys are keywords that give a short description what
    ///  the actual text represents, e.g. Title, Author, Description, or anything else.
    ///
    ///  A keyword is minimum 1 character and maximum 79 characters long. It's
    ///  discouraged to use a single line length longer than 79 characters for texts.
    pub(crate) texts: Vec<LatinText>,

    ///  international text chunks (iTXt)
    ///  Similar to the non-international text chunks, but with additional strings
    ///  "langtags" and "transkeys".
    pub(crate) itexts: Vec<IntlText>,
}

impl Info {
    /// It's supposed to be in UTF-8, but trusting chunk data to be valid would be naive
    pub(crate) fn push_itext(&mut self, key: &[u8], langtag: &[u8], transkey: &[u8], value: &[u8]) -> Result<(), PNGError> {
        self.itexts.push(IntlText {
            key: String::from_utf8_lossy(key).into_owned().into(),
            langtag: String::from_utf8_lossy(langtag).into_owned().into(),
            transkey: String::from_utf8_lossy(transkey).into_owned().into(),
            value: String::from_utf8_lossy(value).into_owned().into(),
        });
        Ok(())
    }

    pub(crate) fn push_text(&mut self, k: &[u8], v: &[u8]) -> Result<(), PNGError> {
        self.texts.push(LatinText {
            key: k.into(),
            value: v.into(),
        });
        Ok(())
    }

    pub fn push_unknown_chunk(&mut self, critical_pos: ChunkPosition, chunk: &[u8]) -> Result<(), PNGError> {
        self.unknown_chunks[critical_pos as usize].try_extend_from_slice(chunk)?;
        Ok(())
    }
}


impl Info {

    #[inline(always)]
    pub fn text_keys(&self) -> TextKeysIter<'_> {
        TextKeysIter { s: &self.texts }
    }

    #[inline(always)]
    pub fn itext_keys(&self) -> ITextKeysIter<'_> {
        ITextKeysIter { s: &self.itexts }
    }

    /// use this to clear the texts again after you filled them in
    #[inline]
    pub fn clear_text(&mut self) {
        self.texts = Vec::new();
        self.itexts = Vec::new();
    }

    /// push back both texts at once
    #[inline]
    pub fn add_text(&mut self, key: &str, str: &str) -> Result<(), PNGError> {
        self.push_text(key.as_bytes(), str.as_bytes())
    }

    /// use this to clear the itexts again after you filled them in
    #[inline]
    pub fn clear_itext(&mut self) {
        self.itexts = Vec::new();
    }

    /// push back the 4 texts of 1 chunk at once
    pub fn add_itext(&mut self, key: &str, langtag: &str, transkey: &str, text: &str) -> Result<(), PNGError> {
        self.push_itext(
            key.as_bytes(),
            langtag.as_bytes(),
            transkey.as_bytes(),
            text.as_bytes(),
        )
    }

    /// Add literal PNG-data chunk unmodified to the unknown chunks
    #[inline]
    pub fn append_chunk(&mut self, position: ChunkPosition, chunk: ChunkRef<'_>) -> Result<(), PNGError> {
        self.unknown_chunks[position as usize].extend_from_slice(chunk.data);
        Ok(())
    }

    /// Uses linear search to find a given chunk. You can use `b"PLTE"` syntax.
    pub fn get<NameBytes: AsRef<[u8]>>(&self, index: NameBytes) -> Option<ChunkRef<'_>> {
        let index = index.as_ref();
        self.try_unknown_chunks(ChunkPosition::IHDR)
            .chain(self.try_unknown_chunks(ChunkPosition::PLTE))
            .chain(self.try_unknown_chunks(ChunkPosition::IDAT))
            .filter_map(|c| c.ok())
            .find(|c| c.is_type(index))
    }

    /// Iterate over chunks that aren't part of image data. Only available if `remember_unknown_chunks` was set.
    #[inline]
    pub fn try_unknown_chunks(&self, position: ChunkPosition) -> ChunksIter<'_> {
        ChunksIter::new(&self.unknown_chunks[position as usize])
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LatinText {
    pub(crate) key: Box<[u8]>,
    pub(crate) value: Box<[u8]>,
}

#[derive(Debug, Clone)]
pub(crate) struct IntlText {
    pub(crate) key: Box<str>,
    pub(crate) langtag: Box<str>,
    pub(crate) transkey: Box<str>,
    pub(crate) value: Box<str>,
}

/// The information of a `Time` chunk in PNG
#[derive(Copy, Clone, Debug, Default)]
pub struct Time {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Color mode of an image. Contains all information required to decode the pixel
/// bits to RGBA colors. This information is the same as used in the PNG file
/// format, and is used both for PNG and raw image data in LodePNG.
#[derive(Clone, Debug)]
pub struct ColorMode {
    /// color type, see PNG standard
    pub colortype: ColorType,
    /// bits per sample, see PNG standard
    pub(crate) bitdepth: u32,

    /// palette (`PLTE` and `tRNS`)
    /// Dynamically allocated with the colors of the palette, including alpha.
    /// When encoding a PNG, to store your colors in the palette of the ColorMode, first use
    /// lodepng_palette_clear, then for each color use lodepng_palette_add.
    /// If you encode an image without alpha with palette, don't forget to put value 255 in each A byte of the palette.
    ///
    /// When decoding, by default you can ignore this palette, since LodePNG already
    /// fills the palette colors in the pixels of the raw RGBA output.
    ///
    /// The palette is only supported for color type 3.
    pub(crate) palette: Option<Box<[RGBA; 256]>>,
    /// palette size in number of colors (amount of bytes is 4 * `palettesize`)
    pub(crate) palettesize: usize,

    /// transparent color key (`tRNS`)
    ///
    /// This color uses the same bit depth as the bitdepth value in this struct, which can be 1-bit to 16-bit.
    /// For greyscale PNGs, r, g and b will all 3 be set to the same.
    ///
    /// When decoding, by default you can ignore this information, since LodePNG sets
    /// pixels with this key to transparent already in the raw RGBA output.
    ///
    /// The color key is only supported for color types 0 and 2.
    pub(crate) key_defined: u32,
    pub(crate) key_r: u32,
    pub(crate) key_g: u32,
    pub(crate) key_b: u32,
}

impl ColorMode {
    #[inline(always)]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[inline(always)]
    #[must_use]
    pub(crate) fn bitdepth(&self) -> u32 {
        self.bitdepth
    }

    #[inline]
    pub(crate) fn set_bitdepth(&mut self, d: u32) {
        assert!(d >= 1 && d <= 16);
        self.bitdepth = d;
    }

    /// Reset to 0 colors
    #[inline]
    pub(crate) fn palette_clear(&mut self) {
        self.palette = None;
        self.palettesize = 0;
    }

    /// add 1 color to the palette
    pub(crate) fn palette_add(&mut self, p: RGBA) -> Result<(), PNGError> {
        if self.palettesize >= 256 {
            return Err(PNGError::new(38));
        }
        let pal = self.palette.get_or_insert_with(|| Box::new([RGBA::default(); 256]));
        pal[self.palettesize] = p;
        self.palettesize += 1;
        Ok(())
    }

    #[inline]
    pub(crate) fn palette(&self) -> &[RGBA] {
        let len = self.palettesize;
        self.palette.as_deref().and_then(|p| p.get(..len)).unwrap_or_default()
    }

    #[inline]
    pub(crate) fn palette_mut(&mut self) -> &mut [RGBA] {
        let len = self.palettesize;
        self.palette.as_deref_mut().and_then(|p| p.get_mut(..len)).unwrap_or_default()
    }

    /// get the total amount of bits per pixel, based on colortype and bitdepth in the struct
    #[inline(always)]
    pub(crate) fn bpp(&self) -> u32 {
        lodepng_get_bpp_lct(self.colortype, self.bitdepth()) /*4 or 6*/
    }

    pub(crate) fn clear_key(&mut self) {
        self.key_defined = 0;
    }

    /// `tRNS` chunk
    #[inline]
    pub fn set_key(&mut self, r: u16, g: u16, b: u16) {
        self.key_defined = 1;
        self.key_r = r as u32;
        self.key_g = g as u32;
        self.key_b = b as u32;
    }

    #[inline]
    pub(crate) fn key(&self) -> Option<(u16, u16, u16)> {
        if self.key_defined != 0 {
            Some((self.key_r as u16, self.key_g as u16, self.key_b as u16))
        } else {
            None
        }
    }

    /// is it a greyscale type? (only colortype 0 or 4)
    #[inline]
    pub(crate) fn is_greyscale_type(&self) -> bool {
        self.colortype == ColorType::GREY || self.colortype == ColorType::GREY_ALPHA
    }

    /// has it got an alpha channel? (only colortype 2 or 6)
    #[inline]
    pub(crate) fn is_alpha_type(&self) -> bool {
        (self.colortype as u32 & 4) != 0
    }

    /// only returns true if there is a palette and there is a value in the palette with alpha < 255.
    /// Loops through the palette to check this.
    #[must_use]
    pub(crate) fn has_palette_alpha(&self) -> bool {
        self.palette().iter().any(|p| p.a < 255)
    }

    /// Check if the given color info indicates the possibility of having non-opaque pixels in the PNG image.
    /// Returns true if the image can have translucent or invisible pixels (it still be opaque if it doesn't use such pixels).
    /// Returns false if the image can only have opaque pixels.
    /// In detail, it returns true only if it's a color type with alpha, or has a palette with non-opaque values,
    /// or if "`key_defined`" is true.
    pub(crate) fn can_have_alpha(&self) -> bool {
        self.key().is_some() || self.is_alpha_type() || self.has_palette_alpha()
    }

    /// Returns the byte size of a raw image buffer with given width, height and color mode
    #[inline]
    pub(crate) fn raw_size(&self, w: u32, h: u32) -> usize {
        self.raw_size_opt(w, h).expect("overflow")
    }

    fn raw_size_opt(&self, w: u32, h: u32) -> Option<usize> {
        let bpp = self.bpp() as usize;
        let n = (w as usize).checked_mul(h as usize)?;
        (n / 8).checked_mul(bpp)?.checked_add(((n & 7) * bpp + 7) / 8)
    }

    /*in an idat chunk, each scanline is a multiple of 8 bits, unlike the lodepng output buffer*/
    pub(crate) fn raw_size_idat(&self, w: usize, h: usize) -> Option<usize> {
        let bpp = self.bpp() as usize;
        let line = (w / 8).checked_mul(bpp)?.checked_add(((w & 7) * bpp + 7) / 8)?;
        h.checked_mul(line)
    }
}

impl Default for ColorMode {
    #[inline]
    fn default() -> Self {
        Self {
            key_defined: 0,
            key_r: 0,
            key_g: 0,
            key_b: 0,
            colortype: ColorType::RGBA,
            bitdepth: 8,
            palette: None,
            palettesize: 0,
        }
    }
}

impl ColorType {
    /// Create color mode with given type and bitdepth
    #[inline]
    pub fn to_color_mode(&self, bitdepth: u32) -> ColorMode {
        ColorMode {
            colortype: *self,
            bitdepth,
            ..ColorMode::default()
        }
    }

    /// channels * bytes per channel = bytes per pixel
    #[inline]
    pub fn channels(&self) -> u8 {
        match *self {
            ColorType::GREY | ColorType::PALETTE => 1,
            ColorType::GREY_ALPHA => 2,
            ColorType::BGR |
            ColorType::RGB => 3,
            ColorType::BGRA |
            ColorType::BGRX |
            ColorType::RGBA => 4,
        }
    }
}

/// Reference to a chunk
#[derive(Copy, Clone)]
pub struct ChunkRef<'a> {
    data: &'a [u8],
}

impl<'a> ChunkRef<'a> {
    #[inline]
    pub(crate) fn new(data: &'a [u8]) -> Result<Self, PNGError> {
        if data.len() < 12 {
            return Err(PNGError::new(30));
        }
        let len = lodepng_chunk_length(data);
        /*error: chunk length larger than the max PNG chunk size*/
        if len > (1 << 31) {
            return Err(PNGError::new(63));
        }
        if data.len() - 12 < len {
            return Err(PNGError::new(64));
        }

        Ok(Self {
            data: &data[0..len + 12],
        })
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        lodepng_chunk_length(self.data)
    }

    /// Chunk type, e.g. `tRNS`
    #[inline]
    pub fn name(&self) -> [u8; 4] {
        let mut tmp = [0; 4];
        tmp.copy_from_slice(&self.data[4..8]);
        tmp
    }

    /// True if `name()` equals this arg
    #[inline]
    pub fn is_type<C: AsRef<[u8]>>(&self, name: C) -> bool {
        self.name() == name.as_ref()
    }

    #[inline]
    pub fn is_ancillary(&self) -> bool {
        (self.data[4] & 32) != 0
    }

    #[inline]
    pub fn is_private(&self) -> bool {
        (self.data[6] & 32) != 0
    }

    #[inline]
    pub fn is_safe_to_copy(&self) -> bool {
        (self.data[7] & 32) != 0
    }

    #[inline]
    pub fn data(&self) -> &[u8] {
        let len = self.len();
        &self.data[8..8 + len]
    }

    #[inline]
    pub fn crc(&self) -> u32 {
        let length = self.len();
        crate::lode_png::crc32fast::hash(&self.data[4..length + 8])
    }

    pub fn check_crc(&self) -> bool {
        let length = self.len();
        /*the CRC is taken of the data and the 4 chunk type letters, not the length*/
        let crc = lodepng_read32bit_int(&self.data[length + 8..]);
        let checksum = self.crc();
        crc == checksum
    }

    /// header + data + crc
    #[inline(always)]
    pub(crate) fn whole_chunk_data(&self) -> &[u8] {
        self.data
    }
}

pub struct ChunkRefMut<'a> {
    data: &'a mut [u8],
}

impl<'a> ChunkRefMut<'a> {
    #[inline]
    pub fn data_mut(&mut self) -> &mut [u8] {
        let len = ChunkRef::new(self.data).unwrap().len();
        &mut self.data[8..8 + len]
    }

    #[inline]
    pub fn generate_crc(&mut self) {
        lodepng_chunk_generate_crc(self.data)
    }
}

/// Position in the file section after…
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum ChunkPosition {
    IHDR = 0,
    PLTE = 1,
    IDAT = 2,
}

/// The settings, state and information for extended encoding and decoding
#[derive(Clone, Debug, Default)]
pub struct State {
    pub decoder: DecoderSettings,
    pub encoder: EncoderSettings,
    /// specifies the format in which you would like to get the raw pixel buffer
    pub info_raw: ColorMode,
    /// info of the PNG image obtained after decoding
    pub info_png: Info,
    pub error: u32,
}


impl State {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline(always)]
    pub fn set_auto_convert(&mut self, mode: bool) {
        self.encoder.auto_convert = mode;
    }

    #[inline(always)]
    pub fn set_filter_strategy(&mut self, mode: FilterStrategy, palette_filter_zero: bool) {
        self.encoder.filter_strategy = mode;
        self.encoder.filter_palette_zero = palette_filter_zero;
    }

    #[inline(always)]
    pub fn info_raw(&self) -> &ColorMode {
        &self.info_raw
    }

    #[inline(always)]
    pub fn info_raw_mut(&mut self) -> &mut ColorMode {
        &mut self.info_raw
    }

    #[inline(always)]
    pub fn info_png_mut(&mut self) -> &mut Info {
        &mut self.info_png
    }

    /// whether to convert the PNG to the color type you want. Default: yes
    #[inline(always)]
    pub fn color_convert(&mut self, true_or_false: bool) {
        self.decoder.color_convert = true_or_false;
    }

    /// if false but `remember_unknown_chunks` is true, they're stored in the unknown chunks.
    #[inline(always)]
    pub fn read_text_chunks(&mut self, true_or_false: bool) {
        self.decoder.read_text_chunks = true_or_false;
    }

    /// store all bytes from unknown chunks in the `Info` (off by default, useful for a png editor)
    #[inline(always)]
    pub fn remember_unknown_chunks(&mut self, true_or_false: bool) {
        self.decoder.remember_unknown_chunks = true_or_false;
    }

    /// Decompress ICC profile from `iCCP` chunk. Only available if `remember_unknown_chunks` was set.
    pub fn get_icc(&self) -> Result<Vec<u8>, PNGError> {
        let iccp = self.info_png.get("iCCP");
        if iccp.is_none() {
            return Err(PNGError::new(89));
        }
        let iccp = iccp.as_ref().unwrap().data();
        if iccp.get(0).cloned().unwrap_or(255) == 0 { // text min length is 1
            return Err(PNGError::new(89));
        }

        let name_len = cmp::min(iccp.len(), 80); // skip name
        for i in 0..name_len {
            if iccp[i] == 0 { // string terminator
                if iccp.get(i+1).cloned().unwrap_or(255) != 0 { // compression type
                    return Err(PNGError::new(72));
                }
                return lodepng_zlib_decompress(&iccp[i+2 ..]);
            }
        }
        Err(PNGError::new(75))
    }
}

#[derive(Clone, Debug)]
pub struct EncoderSettings {
    /// settings for the zlib encoder, such as window size, ...
    //pub zlibsettings: CompressSettings,
    /// how to automatically choose output PNG color type, if at all
    pub auto_convert: bool,
    /// If true, follows the official PNG heuristic: if the PNG uses a palette or lower than
    /// 8 bit depth, set all filters to zero. Otherwise use the filter_strategy. Note that to
    /// completely follow the official PNG heuristic, filter_palette_zero must be true and
    /// filter_strategy must be FilterStrategy::MINSUM
    pub filter_palette_zero: bool,
    /// Which filter strategy to use when not using zeroes due to filter_palette_zero.
    /// Set filter_palette_zero to 0 to ensure always using your chosen strategy. Default: FilterStrategy::MINSUM
    pub filter_strategy: FilterStrategy,

    /// used if filter_strategy is FilterStrategy::PREDEFINED. In that case, this must point to a buffer with
    /// the same length as the amount of scanlines in the image, and each value must <= 5. You
    /// have to cleanup this buffer, LodePNG will never free it. Don't forget that filter_palette_zero
    /// must be set to 0 to ensure this is also used on palette or low bitdepth images
    //pub(crate) predefined_filters: *const u8,

    /// force creating a `PLTE` chunk if colortype is 2 or 6 (= a suggested palette).
    /// If colortype is 3, `PLTE` is _always_ created
    pub force_palette: bool,
    /// add LodePNG identifier and version as a text chunk, for debugging
    pub add_id: bool,
    /// encode text chunks as zTXt chunks instead of tEXt chunks, and use compression in iTXt chunks
    pub text_compression: bool,
}

impl Default for EncoderSettings {
    #[inline]
    fn default() -> Self {
        Self {
            filter_palette_zero: true,
            filter_strategy: FilterStrategy::MINSUM,
            auto_convert: true,
            force_palette: false,
            //predefined_filters: ptr::null_mut(),
            add_id: false,
            text_compression: true,
        }
    }
}

/// Settings for the decoder. This contains settings for the PNG and the Zlib decoder, but not the `Info` settings from the `Info` structs.
#[derive(Clone, Debug, Default)]
pub struct DecoderSettings {
    /// ignore CRC checksums
    pub ignore_crc: bool,
    pub color_convert: bool,
    pub read_text_chunks: bool,
    pub remember_unknown_chunks: bool,
}


pub struct TextKeysIter<'a> {
    pub(crate) s: &'a [LatinText],
}

/// Item is: key value
impl<'a> Iterator for TextKeysIter<'a> {
    /// key value
    type Item = (&'a [u8], &'a [u8]);
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if let Some((first, rest)) = self.s.split_first() {
            self.s = rest;
            Some((&first.key, &first.value))
        } else {
            None
        }
    }
}

pub struct ITextKeysIter<'a> {
    pub(crate) s: &'a [IntlText],
}

/// Item is: key langtag transkey value
impl<'a> Iterator for ITextKeysIter<'a> {
    /// key langtag transkey value
    type Item = (&'a str, &'a str, &'a str, &'a str);
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if let Some((first, rest)) = self.s.split_first() {
            self.s = rest;
            Some((
                &first.key,
                &first.langtag,
                &first.transkey,
                &first.value,
            ))
        } else {
            None
        }
    }
}

/// Iterator of chunk metadata, returns `ChunkRef` which is like a slice of PNG metadata.
/// Stops on the first error. Use `ChunksIter` instead.
pub struct ChunksIterFragile<'a> {
    pub(crate) iter: ChunksIter<'a>,
}

impl<'a> ChunksIterFragile<'a> {
    #[inline(always)]
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            iter: ChunksIter::new(data),
        }
    }
}

impl<'a> ChunksIter<'a> {
    #[inline(always)]
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }
}

impl<'a> Iterator for ChunksIterFragile<'a> {
    type Item = ChunkRef<'a>;
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().and_then(|item| item.ok())
    }
}

/// Iterator of chunk metadata, returns `ChunkRef` which is like a slice of PNG metadata
pub struct ChunksIter<'a> {
    pub(crate) data: &'a [u8],
}

impl<'a> Iterator for ChunksIter<'a> {
    type Item = Result<ChunkRef<'a>, PNGError>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.data.is_empty() {
            return None;
        }
        let ch = match ChunkRef::new(self.data) {
            Ok(ch) => ch,
            Err(e) => return Some(Err(e)),
        };
        self.data = &self.data[ch.len() + 12..];
        Some(Ok(ch))
    }
}
