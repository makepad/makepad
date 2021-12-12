use {
    makepad_render::makepad_live_compiler::LivePtr,
    crate::code_editor::{
        delta::{Delta, OperationRange},
        text::Text,
    },
    std::{
        ops::{Deref, Index},
        slice::Iter,
    },
};

pub struct EditInfoCache {
    pub lines: Vec<Line>,
    pub is_clean:bool,
}

impl EditInfoCache {
    pub fn new(text: &Text) -> EditInfoCache {
        EditInfoCache {
            is_clean:false,
            lines: (0..text.as_lines().len())
                .map(|_| Line::default())
                .collect::<Vec<_>>(),
        }
    }

    pub fn invalidate(&mut self, delta: &Delta) {
        for operation_range in delta.operation_ranges() {
            match operation_range {
                OperationRange::Insert(range) => {
                    self.is_clean = false;
                    self.lines[range.start.line] = Line::default();
                    self.lines.splice(
                        range.start.line..range.start.line,
                        (0..range.end.line - range.start.line).map(|_| Line::default()),
                    );
                }
                OperationRange::Delete(range) => {
                    self.is_clean = false;
                    self.lines.drain(range.start.line..range.end.line);
                    self.lines[range.start.line] = Line::default();
                }
            }
        }
    }

}

impl Deref for EditInfoCache {
    type Target = [Line];

    fn deref(&self) -> &Self::Target {
        &self.lines
    }
}

impl Index<usize> for EditInfoCache {
    type Output = Line;

    fn index(&self, index: usize) -> &Self::Output {
        &self.lines[index]
    }
}

impl<'a> IntoIterator for &'a EditInfoCache {
    type Item = &'a Line;
    type IntoIter = Iter<'a, Line>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive( Debug, Default)]
pub struct Line {
    pub is_clean: bool, 
    pub live_ptrs: Vec<(usize, LivePtr)>
}

impl Line {
}
