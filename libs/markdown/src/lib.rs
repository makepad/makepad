//use makepad_live_id::*;
use std::str::Chars;

#[derive(Default)]
pub struct MarkdownDoc{
    pub decoded: String,
    pub nodes: Vec<MarkdownNode>,
}

#[derive(Debug)]
pub enum MarkdownNode{
    BeginHead{level:usize},
    EndHead,
    BeginItem{count:usize},
    EndItem,
    BeginNormal,
    EndNormal,
    Link{name_start:usize, name_end:usize, url_start:usize, url_end:usize},
    Image{name_start:usize, name_end:usize, url_start:usize, url_end:usize},
    BeginQuote,
    EndQuote,
    BeginCode,
    EndCode,
    BeginInlineCode,
    EndInlineCode,
    BeginBold,
    BeginItalic,
    EndBold,
    EndItalic,
    Text{start:usize, end:usize}
}

#[derive(Clone)]
struct Cursor<'a>{
    iter: Chars<'a>,
    chars:[char;3],
    last_char: char,
}

impl <'a> Cursor<'a>{
    fn new(s:&'a str)->Self{
        let mut ret = Self{
            iter: s.chars(),
            chars: ['\0';3],
            last_char: '\0',
        };
        ret.next();
        ret.next();
        ret.next();
        ret
    }
    
    fn at_end(&self)->bool{
        self.chars[0] == '\0'
    }
    
    fn skip(&mut self, count: usize){
        for _i in 0..count{
            self.next();
        }
    }

    fn next(&mut self){
        self.last_char = self.chars[0];
        self.chars[0] = self.chars[1];
        self.chars[1] = self.chars[2];
        if let Some(c) = self.iter.next(){
            self.chars[2] = c;
        }
        else{
            self.chars[2] = '\0';
        }
    }
}

pub fn parse_markdown(body:&str)->MarkdownDoc{
    let mut nodes = Vec::new();
    let mut decoded = String::new();
    
    let _last_was_ws = false;
    //let mut dstack = Vec::new();
    let mut cursor = Cursor::new(body);
    enum State{
        Root{spaces:usize},
        Inline{head:bool, bold:usize, italic:usize}, // terminates
    }
    let mut state = State::Root{spaces:0};
    
    fn push_char(nodes: &mut Vec<MarkdownNode>, decoded:&mut String, c:char){
        // ok so lets check our last node
        let start = decoded.len();
        decoded.push(c);
        if let Some(last) = nodes.last_mut(){
            if let MarkdownNode::Text{end,..} = last{
                *end = decoded.len()
            }
            else{ 
                nodes.push(MarkdownNode::Text{start, end:decoded.len()})
            }
        }
    }
    
    while !cursor.at_end(){
        match &mut state{
            State::Inline{head, bold, italic}=> match cursor.chars{
                ['\n',_,_]=>{
                    let bold = *bold;
                    let italic = *italic;
                    if *head{
                        cursor.next();
                        nodes.push(MarkdownNode::EndHead);
                        state = State::Root{spaces:0};
                    }
                    else{
                        let last_is_space = cursor.last_char == ' ';
                        cursor.next();
                        while cursor.chars[0] == ' '{
                            cursor.next();
                        }
                        if cursor.chars[0] == '\n'{
                            cursor.next();
                            state = State::Root{spaces:0};
                            nodes.push(MarkdownNode::EndNormal);
                        }
                        else if !last_is_space{
                            push_char(&mut nodes, &mut decoded, ' ');
                        }
                    }
                    // TODO: clean up unmatched bolds and italics properly
                    if let State::Root{..} = state{
                        for _ in 0..bold{
                            nodes.push(MarkdownNode::EndBold);
                        }
                        for _ in 0..italic{
                            nodes.push(MarkdownNode::EndItalic);
                        }
                    }
                }
                ['*','*',w] | ['_','_',w] if w != ' ' && w != '\n'=>{ // alright so have have 2 *'s
                    // this is the start of a bold block
                    nodes.push(MarkdownNode::BeginBold);
                    *bold += 1;
                    cursor.skip(2);
                }
                [w,'*','*'] | [w,'_','_'] if w != ' '&& w != '\n'=>{
                    // end of a bold block
                    push_char(&mut nodes, &mut decoded, w);
                    if *bold > 0{
                        *bold -= 1;
                        cursor.skip(3);
                        nodes.push(MarkdownNode::EndBold);
                    }
                    else{
                        cursor.next();
                    }
                }
                ['*',w,_] | ['_',w,_] if w != ' ' && w != '\n'=>{
                    *italic += 1;
                    nodes.push(MarkdownNode::BeginItalic);
                    cursor.skip(1);
                }
                [w,'*',_] | [w,'_',_] if w != ' ' && w != '\n' =>{
                    push_char(&mut nodes, &mut decoded, w);
                    cursor.skip(2);
                    if *italic > 0{
                        *italic -= 1;
                        nodes.push(MarkdownNode::EndItalic);
                    }
                    else{
                        push_char(&mut nodes, &mut decoded, '*');
                    }
                }
                ['*',_,_] =>{
                    if *italic > 0 && cursor.last_char == '*'{
                        *italic -= 1;
                        cursor.skip(1);
                        nodes.push(MarkdownNode::EndItalic);
                    }
                    else{
                        push_char(&mut nodes, &mut decoded, '*');
                        cursor.skip(1);
                    }
                }
                ['_',_,_] =>{
                    if *italic > 0 && cursor.last_char == '_'{
                        *italic -= 1;
                        cursor.skip(1);
                        nodes.push(MarkdownNode::EndItalic);
                    }
                    else{
                        push_char(&mut nodes, &mut decoded, '_');
                        cursor.skip(1);
                    }
                } 
                                
                ['`','`','`'] =>{ // big code block
                    nodes.push(MarkdownNode::EndHead);
                    state = State::Root{spaces:0};
                }
                ['`',_,_] =>{ // inline code block
                    let mut scan = cursor.clone();
                    scan.skip(1);
                    let start = decoded.len();
                    while scan.chars[0] != '`' && !scan.at_end(){
                        if scan.chars[0] == '\n' && scan.last_char == '\n'{
                            break; // double newline terminates inline block
                        }
                        decoded.push(scan.chars[0]);
                        scan.next();
                    }
                    if scan.chars[0] == '`'{
                        nodes.push(MarkdownNode::BeginInlineCode);
                        nodes.push(MarkdownNode::Text{start, end:decoded.len()});
                        nodes.push(MarkdownNode::EndInlineCode);
                        scan.next();
                        cursor = scan;
                    }
                    else{
                        decoded.truncate(start);
                        push_char(&mut nodes, &mut decoded, '`');
                        cursor.next();
                    }
                }
                ['!', '[', _]=>{
                    // parse inline image
                    cursor.next();
                }
                ['[',_,_]=>{ // possible named link
                    cursor.next();
                }
                [' ',_,_]=>{
                    if cursor.last_char != ' '{
                        push_char(&mut nodes, &mut decoded, ' ');
                    }
                    cursor.next();
                }
                _=>{
                    push_char(&mut nodes, &mut decoded, cursor.chars[0]);
                    cursor.next();
                }
            }
            State::Root{spaces} => match cursor.chars{
                [' ',_,_]=>{ // space counter
                    state = State::Root{spaces:*spaces + 1};
                    cursor.skip(1)
                }
                ['>',_,_]=>{
                    if *spaces>4{ // its code
                        //state = State::InlineCode;
                       // decoded.push('>');
                    }
                    else{ // its a quote block, lets cound more >s
                        
                    }
                }
                ['#',_,_]=>{
                    let mut level = 0;
                    let start = decoded.len();
                    while cursor.chars[0] == '#'{
                        level += 1;
                        if level>6{
                            break;
                        }
                        cursor.next();
                        decoded.push('#');
                    }
                    if level>6 || cursor.chars[0] != ' '{
                        // lets append
                        if let Some(MarkdownNode::Text{end,..}) = nodes.last_mut(){
                            *end = decoded.len();
                        }
                        else{
                            nodes.push(MarkdownNode::Text{start, end:decoded.len()});
                        }
                        state = State::Inline{head:false, bold:0, italic:0};
                    }
                    else {
                        cursor.next();
                        decoded.truncate(start);
                        nodes.push(MarkdownNode::BeginHead{level});
                        state = State::Inline{head:true, bold:0, italic:0};
                    }
                }
                ['`','`','`']=>{ // begins or ends blocks of code. 
                    let mut scan = cursor.clone();
                    scan.skip(3);
                    let start = decoded.len();
                    while scan.chars != ['`','`','`'] && !scan.at_end(){
                        decoded.push(scan.chars[0]);
                        scan.skip(1);
                    }
                    if !scan.at_end(){
                        nodes.push(MarkdownNode::BeginCode);
                        nodes.push(MarkdownNode::Text{start, end:decoded.len()});
                        nodes.push(MarkdownNode::EndCode);
                        scan.skip(3); // skip last
                        cursor = scan;
                    }
                    else{
                        decoded.truncate(start);
                    }
                }
                /*
                ['-',_,_]=>{ // possible list item
                                    
                }
                ['+',_,_]=>{ // possible list item
                                    
                }
                ['*',_,_]=>{ // possible list item
                                                    
                }
                ['|',_,_]=>{ // table
                                    
                }*/
                ['\n',_,_]=>{ // skip it
                    cursor.skip(1);
                }
                [_a,_b,_c]=>{
                    // parse if numbered list
                    // otherwise this is a normal text block
                    nodes.push(MarkdownNode::BeginNormal);
                    state = State::Inline{head:false, bold:0, italic:0};
                    nodes.push(MarkdownNode::EndNormal);
                }
            }
        }
    }
    MarkdownDoc{
        nodes,
        decoded,
    }
}