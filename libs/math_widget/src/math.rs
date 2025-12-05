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
        draw_math: {
            texture tex: texture2d
        }
    }
}

#[derive(Live, LiveHook, Widget)]
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

    #[rust]
    old_text: String,
    #[rust]
    world: MathWorld,
    #[rust]
    texture: Option<Texture>,
}

impl Widget for Math {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, mut walk: Walk) -> DrawStep {
        self.render_math(cx);    
        if let Some(texture) = &self.texture {
            let (width, height) = texture.get_format(cx).vec_width_height().unwrap_or((0, 0));
            walk.width = Size::Fixed(width as f64);
            walk.height = Size::Fixed(height as f64);
            self.draw_math.draw_vars.set_texture(0, texture);
            self.draw_math.draw_walk(cx, walk);
        }
        DrawStep::done()
    }
}

impl Math {
    fn render_math(&mut self, cx: &mut Cx) {
        if self.text == self.old_text {
            return;
        }
        let header = r#"
            #set page(width: auto, height: auto, margin: 0pt, fill: none)
            #set text(fill: white)
        "#;
        let full_text = format!("{}{}", header, self.text);
        self.world.set_text(&full_text);
        self.old_text = self.text.clone();

        match typst::compile::<PagedDocument>(&self.world).output {
            Ok(document) => {
                if let Some(page) = document.pages.first() {
                    let pixmap = typst_render::render(page, 2.0);
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
                    
                    self.walk.width = Size::Fixed(width as f64 / 2.0);
                    self.walk.height = Size::Fixed(height as f64 / 2.0);
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