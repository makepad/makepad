use {
    crate::code_editor::{
        delta::{Delta, OperationRange},
        text::Text,
        token::TokenKind,
        token_cache::TokenCache,
    },
    std::{
        ops::{Deref, Index},
        slice::Iter,
    },
    makepad_render::makepad_live_compiler::{TextPos, LivePtr},
    makepad_render::*,
};

#[derive(Debug)]
pub struct LineItem {
    pub live_ptr: LivePtr,
    pub edit_token_index: usize,
}

#[derive( Debug, Default)]
pub struct Line {
    pub is_clean: bool, 
    pub items: Vec<LineItem>
}

pub struct LiveEditCache {
    pub lines: Vec<Line>,
    pub is_clean:bool,
}

impl LiveEditCache {
    pub fn new(text: &Text) -> LiveEditCache {
        LiveEditCache {
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

    pub fn refresh(&mut self, token_cache: &TokenCache, cx: &mut Cx) {
        if self.is_clean {
            return
        }
        self.is_clean = true;
        
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        
        let file_id = LiveFileId(10);
        
        let live_file = &live_registry.live_files[file_id.to_index()];
        let expanded = &live_registry.expanded[file_id.to_index()];
        
        if self.lines.len() != token_cache.len() {
            panic!();
        }
        for (line, line_cache) in self.lines.iter_mut().enumerate() {
            if line_cache.is_clean { // line not dirty
                continue
            }
            line_cache.is_clean = true;
            if line_cache.items.len() != 0 { 
                panic!();
            }
            let tokens_line = &token_cache[line];
            let mut column = 0;
            for (edit_token_index, token) in tokens_line.tokens().iter().enumerate() {
                if let TokenKind::Identifier = token.kind {
                    if let Some(live_token_index) = live_file.document.find_token_by_pos(TextPos {line: line as u32, column}) {
                        let match_token_id = makepad_live_compiler::TokenId::new(file_id, live_token_index);
                        if let Some(node_index) = expanded.nodes.first_node_with_token_id(match_token_id) {
                            let live_ptr = LivePtr {file_id, index: node_index as u32};
                            
                            line_cache.items.push(LineItem{
                                live_ptr,
                                edit_token_index:edit_token_index
                            });
                        }
                    }
                }
                column += token.len as u32;
            }
        }
    }
}


impl Deref for LiveEditCache {
    type Target = [Line];

    fn deref(&self) -> &Self::Target {
        &self.lines
    }
}

impl Index<usize> for LiveEditCache {
    type Output = Line;

    fn index(&self, index: usize) -> &Self::Output {
        &self.lines[index]
    }
}

impl<'a> IntoIterator for &'a LiveEditCache {
    type Item = &'a Line;
    type IntoIter = Iter<'a, Line>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl Line {
}
