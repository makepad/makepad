//! `ttf_parser::Face` wrapper owning (instead of borrowing) font bytes, similar
//! to what the `owned_ttf_parser` crate offers, but expandable to `rustybuzz`,
//! and using `Rc<Vec<u8>>` instead of `Vec<u8>` (to avoid cloning any bytes).

use makepad_vector::ttf_parser::Face;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::rc::Rc;

pub use makepad_vector::ttf_parser::FaceParsingError;

pub struct OwnedFace(Pin<Box<FaceWithFontData>>);

impl OwnedFace {
    pub fn parse(
        font_data: Rc<Vec<u8>>,
        index_in_collection: u32,
    ) -> Result<Self, FaceParsingError> {
        let mut pinned_box = Box::pin(FaceWithFontData {
            face: None,
            font_data,
            _marker: PhantomPinned,
        });
        pinned_box
            .as_mut()
            .with_face_slot_mut_and_font_data(|face_slot, font_data| {
                let ttf_parser_face = Face::parse(font_data, index_in_collection)?;
                *face_slot = Some(ttf_parser_face);
                Ok(())
            })?;
        Ok(Self(pinned_box))
    }

    pub fn with_ref<R>(&self, f: impl for<'a> FnOnce(&Face<'a>) -> R) -> R {
        self.0.as_ref().with_face_ref(f)
    }
}

struct FaceWithFontData {
    // HACK(eddyb) `'static` here is technically a lie, but should not be an
    // issue as long as we always use HRTB generativity for all accesses.
    face: Option<Face<'static>>,
    font_data: Rc<Vec<u8>>,
    _marker: PhantomPinned,
}

impl FaceWithFontData {
    fn with_face_slot_mut_and_font_data<R>(
        self: Pin<&mut Self>,
        f: impl for<'a> FnOnce(&mut Option<Face<'a>>, &'a [u8]) -> R,
    ) -> R {
        let (face_slot, font_data) = unsafe {
            let self_mut = self.get_unchecked_mut();
            let face_slot = std::mem::transmute::<&mut Option<Face<'static>>, &mut Option<Face<'_>>>(
                &mut self_mut.face,
            );
            (face_slot, &self_mut.font_data[..])
        };
        f(face_slot, font_data)
    }

    fn with_face_ref<R>(self: Pin<&Self>, f: impl for<'a> FnOnce(&Face<'a>) -> R) -> R {
        f(self.face.as_ref().unwrap())
    }
}

impl Drop for FaceWithFontData {
    fn drop(&mut self) {
        // Drop `self.face` early, so no destructors that borrow `self.font_data`
        // can possibly ever run after `self.font_data` is dropped.
        drop(self.face.take());
    }
}
