use makepad_render::*;
use makepad_widget::*;
use crate::searchindex::*;
use crate::makepadstorage::*;
use crate::mprstokenizer::*;
use crate::liveitems::*;

#[derive(Clone)]
pub struct RustEditor {
    pub view: View,
    pub live_items_view: LiveItemsView,
    pub splitter: Splitter,
    pub text_editor: TextEditor,
}

impl RustEditor {
    pub fn new(cx: &mut Cx) -> Self {
        let editor = Self {
            view: View::new(),
            live_items_view: LiveItemsView::new(cx),
            splitter: Splitter {
                pos: 125.0,
                align: SplitterAlign::Last,
                _hit_state_margin: Some(Margin {
                    l: 3.,
                    t: 0.,
                    r: 7.,
                    b: 0.,
                }),
                ..Splitter::new(cx)
            },
            text_editor: TextEditor::new(cx),
        };
        //tab.animator.default = tab.anim_default(cx);
        editor 
    }
    
    pub fn handle_rust_editor(&mut self, cx: &mut Cx, event: &mut Event, mtb: &mut MakepadTextBuffer, search_index: Option<&mut SearchIndex>) -> TextEditorEvent {
        
        self.live_items_view.handle_live_items(cx, event, mtb);
          
        if mtb.live_items_list.visible_editors{
            match self.splitter.handle_splitter(cx, event) {
                SplitterEvent::Moving {..} => {
                    self.view.redraw_view_parent(cx);
                },
                _ => ()
            }
        }
        
        let ce = self.text_editor.handle_text_editor(cx, event, &mut mtb.text_buffer);
        match ce {
            TextEditorEvent::Change => {
                Self::update_token_chunks(cx, mtb, search_index);
            },
            TextEditorEvent::AutoFormat => {
                let formatted = MprsTokenizer::auto_format(&mtb.text_buffer.flat_text, &mtb.text_buffer.token_chunks, false).out_lines;
                self.text_editor.cursors.replace_lines_formatted(formatted, &mut mtb.text_buffer);
                self.text_editor.view.redraw_view(cx);
            },
            _ => ()
        }
        ce
    }
    
    pub fn draw_rust_editor(&mut self, cx: &mut Cx, mtb: &mut MakepadTextBuffer, search_index: Option<&mut SearchIndex>) {
        if self.view.begin_view(cx, Layout::default()).is_err() {
            return
        }; 
        
        if mtb.live_items_list.visible_editors{ 
            self.splitter.begin_splitter(cx); 
        }
            
        Self::update_token_chunks(cx, mtb, search_index);
        
        if self.text_editor.begin_text_editor(cx, &mut mtb.text_buffer).is_ok() {
            for (index, token_chunk) in mtb.text_buffer.token_chunks.iter_mut().enumerate() {
                self.text_editor.draw_chunk(cx, index, &mtb.text_buffer.flat_text, token_chunk, &mtb.text_buffer.markers);
            }
            
            self.text_editor.end_text_editor(cx, &mut mtb.text_buffer);
        }
        if mtb.live_items_list.visible_editors{
            
            self.splitter.mid_splitter(cx);

            self.live_items_view.draw_live_items(cx, mtb, &mut self.text_editor);  

            self.splitter.end_splitter(cx);
        }
        self.view.end_view(cx);
    }
    
    pub fn update_token_chunks(cx: &mut Cx, mtb: &mut MakepadTextBuffer, mut search_index: Option<&mut SearchIndex>) {
        
        if mtb.text_buffer.needs_token_chunks() && mtb.text_buffer.lines.len() >0 {
            
            let mut state = TokenizerState::new(&mtb.text_buffer.lines);
            let mut tokenizer = MprsTokenizer::new();
            let mut pair_stack = Vec::new();
            loop {
                let offset = mtb.text_buffer.flat_text.len();
                let token_type = tokenizer.next_token(&mut state, &mut mtb.text_buffer.flat_text, &mtb.text_buffer.token_chunks);
                if TokenChunk::push_with_pairing(&mut mtb.text_buffer.token_chunks, &mut pair_stack, state.next, offset, mtb.text_buffer.flat_text.len(), token_type) {
                    mtb.text_buffer.was_invalid_pair = true;
                }
                
                if token_type == TokenType::Eof {
                    break
                }
                if let Some(search_index) = search_index.as_mut() {
                    search_index.new_rust_token(&mtb);
                }
            }
            if pair_stack.len() > 0 {
                mtb.text_buffer.was_invalid_pair = true;
            }
            
            // lets parse and generate our live macro set
            // check if our last undo entry isnt LiveEdit
            //let parse_live = if atb.text_buffer.undo_stack.len() != 0 {
            //    if let TextUndoGrouping::LiveEdit(_) = atb.text_buffer.undo_stack.last().unwrap().grouping {false} else {true}
            //}else {true};
            //if parse_live{
            //    if atb.text_buffer.undo_stack.len() != 0{
            //        println!("PARSE LIVE {:?}", atb.text_buffer.undo_stack.last().unwrap().grouping);
            //    }
            mtb.parse_live_bodies(cx);
            //}
            
            // ok now lets write a diff with the previous one
            /*
            let mut new_index = 0;
            let mut old_index = 0;
            let mut recompile = false;
            let mut macro_index = 0;
            loop {
                if let TokenType::Macro = new_tok.token_type {
                    if tok_cmp("pick", new_tok_slice) {
                        // lets parse the new one
                    }
                    // jump new and old to the end of the macro so diffing can continue
                }
                
                if new_index < atb.text_buffer.token_chunks.len() {
                    new_index += 1;
                }
                else {
                    break
                }
                if old_index + 1 < atb.text_buffer.old_token_chunks.len() {
                    old_index += 1;
                    // lets compare the token at this point
                    let new_tok = &atb.text_buffer.token_chunks[new_index];
                    let new_tok_slice = &atb.text_buffer.flat_text[new_tok.offset..new_tok.offset + new_tok.len];
                    let old_tok = &atb.text_buffer.old_token_chunks[old_index];
                    let old_tok_slice = &atb.text_buffer.flat_text[old_tok.offset..old_tok.offset + old_tok.len];
                    if new_tok_slice != old_tok_slice { // things are different and require a recompile
                        recompile = true;
                    }
                }
                else {
                    recompile = true;
                }
            }*/
        }
    }
}
