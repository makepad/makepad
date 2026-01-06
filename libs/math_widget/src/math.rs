/// Math widget for rendering LaTeX equations using Typst.
///
/// DSL Usage:
/// ```
/// <Math> {
///     width: Fit,
///     height: Fit,
///     color: #fff,
///     font_size: 11.0,
///     text: "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}"
/// }
/// ```
///
/// Properties:
/// - `text` - LaTeX math expression to render
/// - `color` - Text color (default: #fff)
/// - `font_size` - Font size in points (default: 11.0)
/// - `baseline_offset` - Vertical offset for baseline alignment (default: -2.0)

use {
    makepad_widgets::*,
    typst::{
        diag::{FileError, FileResult},
        foundations::{Bytes, Datetime},
        layout::PagedDocument,
        syntax::{FileId, Source},
        text::{Font, FontBook},
        utils::LazyHash,
        Library,
        World,
    }
};

live_design!{
    link widgets;

    use link::shaders::*;
    
    DrawMath = {{DrawMath}} {
        texture tex: texture2d

        fn pixel(self) -> vec4 {
            return sample2d(self.tex, self.pos);
        }
    }
    
    pub Math = {{Math}} {
        color: #fff,
        font_size: 11.0,
        baseline_offset: -2.0,
        draw_math: {
            texture tex: texture2d
        }
    }
}

#[derive(Live, Widget)]
pub struct Math {
    #[live]
    draw_math: DrawMath,

    #[redraw]
    #[rust]
    area: Area,
    #[walk]
    walk: Walk,

    #[live]
    text: String,

    #[live]
    color: Vec4,

    #[live]
    font_size: f64,

    #[live]
    baseline_offset: f64,

    #[rust]
    old_text: String,
    #[rust]
    old_color: Vec4,
    #[rust]
    old_font_size: f64,
    #[rust]
    old_dpi_factor: f64,
    #[rust]
    world: MathWorld,
    #[rust]
    texture: Option<Texture>,
}

impl LiveHook for Math {
    fn after_apply(&mut self, cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        // Reset cached values to force re-render when properties change
        self.old_text.clear();
        self.old_color = Vec4::default();
        self.old_font_size = 0.0;
        self.old_dpi_factor = 0.0;
        self.redraw(cx);
    }
}

impl Widget for Math {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, mut walk: Walk) -> DrawStep {
        self.render_math(cx);
        if let Some(texture) = &self.texture {
            walk.width = self.walk.width;
            walk.height = self.walk.height;
            walk.margin.top += self.baseline_offset;
            self.draw_math.draw_vars.set_texture(0, texture);
            self.draw_math.draw_walk(cx, walk);
        }
        DrawStep::done()
    }

    fn set_text(&mut self, cx: &mut Cx, text: &str) {
        self.text = text.to_string();
        self.redraw(cx);
    }
}

impl Math {
    fn render_math(&mut self, cx: &mut Cx2d) {
        let dpi_factor = cx.current_dpi_factor();
        if self.text == self.old_text
            && self.color == self.old_color
            && self.font_size == self.old_font_size
            && dpi_factor == self.old_dpi_factor
        {
            return;
        }
        // Convert color to typst rgb format
        let r = (self.color.x * 255.0) as u8;
        let g = (self.color.y * 255.0) as u8;
        let b = (self.color.z * 255.0) as u8;
        // Scale factor to match Makepad font sizes to typst font sizes
        // roughly ~1.75 ratio
        const FONT_SIZE_SCALE: f64 = 1.75;
        let font_size_pt = self.font_size * FONT_SIZE_SCALE;
        let header = format!(r#"
            #set page(width: auto, height: auto, margin: 0pt, fill: none)
            #set text(fill: rgb({}, {}, {}), size: {}pt)
            #let mitexsqrt = math.sqrt
            #let frac(x, y) = $ (#x)/(#y) $
        "#, r, g, b, font_size_pt);
        let typst_text = mitex::convert_math(&self.text, None).unwrap_or_else(|e| {
             log!("Mitex error: {:?}", e);
             format!("$ \"Error: {}\" $", e)
        });
        let full_text = format!("{}$ {} $", header, typst_text);
        self.world.set_text(&full_text);
        self.old_text = self.text.clone();
        self.old_color = self.color;
        self.old_font_size = self.font_size;
        self.old_dpi_factor = dpi_factor;

        match typst::compile::<PagedDocument>(&self.world).output {
            Ok(document) => {
                if let Some(page) = document.pages.first() {
                    let pixmap = typst_render::render(page, dpi_factor as f32);
                    let width = pixmap.width() as usize;
                    let height = pixmap.height() as usize;
                    let rgba_data = pixmap.data();
                    
                    let mut bgra_data = Vec::with_capacity(width * height);
                    for chunk in rgba_data.chunks(4) {
                        let r = chunk[0] as u32;
                        let g = chunk[1] as u32;
                        let b = chunk[2] as u32;
                        let a = chunk[3] as u32;
                        
                        let pixel = (a << 24) | (r << 16) | (g << 8) | b;
                        bgra_data.push(pixel);
                    }
                    
                    let texture = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
                        data: Some(bgra_data),
                        width,
                        height,
                        updated: TextureUpdated::Full,
                    });
                    
                    self.texture = Some(texture);

                    self.walk.width = Size::Fixed(width as f64 / dpi_factor);
                    self.walk.height = Size::Fixed(height as f64 / dpi_factor);
                }
            }
            Err(errors) => {
                for error in errors {
                    log!("Typst error: {:?}", error);
                }
            }
        }
    }
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawMath {
    #[deref]
    draw_super: DrawQuad,
}

pub struct MathWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    source: Source,
    fonts: Vec<Font>,
}

impl MathWorld {
    fn new() -> Self {
        let font_data = include_bytes!("NewCMMath-Regular.otf");
        let font = Font::new(Bytes::new(font_data.as_slice()), 0).expect("Failed to load font");
        let fonts = vec![font];
        let book = FontBook::from_fonts(&fonts);
        
        Self {
            library: LazyHash::new(Library::default()),
            book: LazyHash::new(book),
            source: Source::detached(""),
            fonts,
        }
    }

    fn set_text(&mut self, text: &str) {
        self.source.replace(text);
    }
}

impl Default for MathWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl World for MathWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.source.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id != self.source.id() {
            return Err(FileError::NotFound(id.vpath().as_rooted_path().into()));
        }
        Ok(self.source.clone())
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        Err(FileError::NotFound(id.vpath().as_rooted_path().into()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<i64>) -> Option<Datetime> {
        None
    }
}