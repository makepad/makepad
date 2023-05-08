use crate::{Diff, Sel, Text};

#[derive(Debug)]
pub struct Context<'a> {
    pub sel: &'a Sel,
}

impl<'a> Context<'a> {
    pub fn insert(&self, text: Text) -> Diff {
        use crate::diff::Builder;

        let mut builder = Builder::new();
        for span in self.sel.spans() {
            if span.is_sel {
                builder.delete(span.len);
                builder.insert(text.clone());
            } else {
                builder.retain(span.len);
            }
        }
        builder.finish()
    }
}
