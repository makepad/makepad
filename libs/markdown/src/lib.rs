//use makepad_live_id::*;
use std::str::Chars;

#[derive(Debug)]
pub struct MarkdownError{
    pub message:String,
    pub position:usize,
}

#[derive(Default)]
pub struct MarkdownDoc{
    pub decoded: String,
    pub nodes: Vec<MarkdownNode>,
}

pub enum MarkdownNode{
    BeginHead,
    EndHead,
    BeginList,
    EndList,
    BeginQuote,
    EndQuote,
    BeginCode,
    EndCode,
    BeginBold,
    BeginItalic,
    EndBold,
    EndItalic,
    Text(usize, usize)
}

struct Cursor<'a>{
    iter: Chars<'a>,
    chars:[char;3],
    pos:[usize;3],
}

 pub fn parse_markdown(body:&str, _errors:  &mut Option<Vec<MarkdownError>>)->MarkdownDoc{
     enum State{
         Begin(usize),
         H(usize, usize),
         Star(usize),
         Underscore(usize),
         Bold(),
         Text(usize),
     }
     
     let nodes = Vec::new();
     let mut state = State::Begin(0);
     let decoded = String::new();
     let _last_was_ws = false;
    // let mut boldstack = Vec::new();
     
     let iter = body.char_indices();
    /* for (_i, c) in body.char_indices(){
         // alright so we do 3 characters at a time
         ('*','*',c) if !c.is_Whitespace(){
             // we emit * * and eat 2
         }
     }*/
     /*   
         // alright if we have ** we insert a bold
         // however we have to check bold-pairing per-line
         state = match state{
             State::Begin(start)=>{
                 // lets count #'s
                 if c == '#'{
                     State::H(start, 1)
                 }
                 else{
                    State::Text(start)
                }
            }
            State::H(start, num)=> if c == '#'{
                if num > 6{
                    State::Text(start)
                }
                else{
                    State::H(start, num+1)
                }
            }
            State::Star(start)=>{
                // for star we replace a last emitted star
                if let Some(MarkDownNode::BeginItalic) = nodes.last(){
                    *nodes.last_mut() = MarkDownNode::BeginBold
                }
            }
            else{
                // alright so. lets emit a H
                State::Text(start)
            }
            State::Text(start)=>{
                if c == '\n'{ // we might end 
                }
            }
        }
        state = match state{
            State::Text(start)=>{
                if c == '*'{
                    nodes.push(MarkDownNode::BeginItalic);
                    State::Star(start)
                }
                else if c == '_'{
                    State::Underscore(start)
                }
                else{
                    State::Text(start)
                }
            },
            x=>x
        };
     }*/
     MarkdownDoc{
         nodes,
         decoded,
     }
 }
 