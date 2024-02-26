//use makepad_live_id::*;

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
 }
 

 pub fn parse_markdown(body:&str, _errors:  &mut Option<Vec<MarkdownError>>)->MarkdownDoc{
     enum State{
         Begin(usize),
         H1,
         H2,
         H3,
         H4,
         H5,
         H6
     }
     
     let nodes = Vec::new();
     let mut state = State::Begin(0);
     let decoded = String::new();
     let _last_was_ws = false;
     
     for (_i, c) in body.char_indices(){
         state = match state{
             State::Begin(start)=>{ 
                 // lets count #'s
                 if c == '#'{
                     State::H1
                 }
                 else{
                    State::Begin(start)
                }
            }
            State::H1=> if c == '#'{
                State::H2
            }
            else{
                State::Begin(0)
            }
            State::H2=> if c == '#'{
                State::H3
            }
            else{
                State::Begin(0)
            }
            State::H3=> if c == '#'{
                State::H4
            }
            else{
                State::Begin(0)
            }
            State::H4=> if c == '#'{
                State::H5
            }
            else{
                State::Begin(0)
            }
            State::H5=> if c == '#'{
                State::H6
            }
            else{
                State::Begin(0)
            }
            State::H6=> if c == '#'{
                State::H6
            }
            else{
                State::Begin(0)
            }
         }
     }
     MarkdownDoc{
         nodes,
         decoded,
     }
 }
 