use {
    makepad_rustybuzz as rustybuzz,
    rustybuzz::ttf_parser,
    std::{fmt, marker::PhantomPinned, mem, pin::Pin, rc::Rc},
};

#[derive(Debug)]
pub struct FontFace(Pin<Box<FontFaceInner>>);

impl FontFace {
    pub fn from_data_and_index(data: Rc<Vec<u8>>, index: u32) -> Option<Self> {
        let mut inner = Box::pin(FontFaceInner {
            data,
            ttf_parser_face: None,
            rustybuzz_face: None,
            _pinned: PhantomPinned,
        });
        unsafe {
            let data: &'static [u8] = mem::transmute(&**inner.data);
            let ttf_parser_face = ttf_parser::Face::parse(data, index).ok()?;
            let rustybuzz_face = rustybuzz::Face::from_face(ttf_parser_face.clone());
            let inner_ref = Pin::as_mut(&mut inner).get_unchecked_mut();
            inner_ref.ttf_parser_face = Some(ttf_parser_face);
            inner_ref.rustybuzz_face = Some(rustybuzz_face);
        }
        Some(Self(inner))
    }

    pub fn as_ttf_parser_face(&self) -> &ttf_parser::Face<'_> {
        self.0.ttf_parser_face.as_ref().unwrap()
    }

    pub fn as_rustybuzz_face(&self) -> &rustybuzz::Face<'_> {
        self.0.rustybuzz_face.as_ref().unwrap()
    }
}

struct FontFaceInner {
    data: Rc<Vec<u8>>,
    ttf_parser_face: Option<ttf_parser::Face<'static>>,
    rustybuzz_face: Option<rustybuzz::Face<'static>>,
    _pinned: PhantomPinned,
}

impl fmt::Debug for FontFaceInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FontFaceInner").finish_non_exhaustive()
    }
}
