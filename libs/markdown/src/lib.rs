//use makepad_live_id::*;
use std::str::Chars;

#[derive(Default)]
pub struct MarkdownDoc{
    pub decoded: String,
    pub nodes: Vec<MarkdownNode>,
}
#[derive(Debug)]
pub enum MarkdownListLabel{
    Plus,
    Minus,
    Star,
    Number{digit:usize, start:usize, end:usize},
}
#[derive(Debug)]
pub enum MarkdownNode{
    BeginHead{level:usize},
    EndHead,
    BeginListItem{label:MarkdownListLabel},
    EndListItem,
    BeginNormal,
    EndNormal,
    Link{start:usize, url_start:usize, end:usize},
    Image{start:usize, url_start:usize, end:usize},
    BeginQuote,
    EndQuote,
    BeginCode,
    EndCode,
    BeginInlineCode,
    NewLine,
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
        Inline{kind:Kind, bold:usize, italic:usize}, // terminates
    }
    enum Kind{
        Normal,
        Head,
        Quote(usize),
        List(usize)
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
    
    fn push_optional_char(nodes: &mut Vec<MarkdownNode>, decoded:&mut String, c:char){
        // ok so lets check our last node
        if let Some(last) = nodes.last_mut(){
            if let MarkdownNode::Text{end,..} = last{
                decoded.push(c);
                *end = decoded.len()
            }
        }
    }
    
    fn code_on_one_line(nodes: &mut Vec<MarkdownNode>, decoded:&mut String, cursor:&mut Cursor){
        // alright we have to check if we are in a code block already
        let already_in_code = if let Some(MarkdownNode::EndCode) = nodes.last(){
            nodes.pop();
            true
        }
        else{false};
        
        let start = decoded.len();
        while !cursor.at_end() && cursor.chars[0] != '\n'{
            decoded.push(cursor.chars[0]);
            cursor.next();
        }
        if !cursor.at_end(){
            cursor.next();
        }
        if !already_in_code{
            nodes.push(MarkdownNode::BeginCode);
        }
        else{
            nodes.push(MarkdownNode::NewLine);
        }
        nodes.push(MarkdownNode::Text{start, end:decoded.len()});
        nodes.push(MarkdownNode::EndCode);
    }
    
    loop{
        match &mut state{
            State::Inline{kind, bold, italic}=> match cursor.chars{
                [' ',' ','\n']=>{
                    nodes.push(MarkdownNode::NewLine);
                    cursor.skip(2);
                }
                ['\n',_,_] | ['\0',_,_]=>{
                    for _ in 0..*bold{
                        nodes.push(MarkdownNode::EndBold);
                    }
                    for _ in 0..*italic{
                        nodes.push(MarkdownNode::EndItalic);
                    }
                    *bold = 0;
                    *italic = 0;
                    
                    match kind{
                        Kind::Head=>{
                            cursor.next();
                            nodes.push(MarkdownNode::EndHead);
                            state = State::Root{spaces:0};
                        }
                        Kind::Quote(blocks)=>{
                            let last_is_space = cursor.last_char == ' ';
                            cursor.next();
                            let mut spaces = 0;
                            while cursor.chars[0] == ' '{
                                cursor.next();
                                spaces += 1;
                            }
                            if cursor.chars[0] == '\n' || cursor.chars[0] == '\0' {
                                cursor.next();
                                for _ in 0..*blocks{
                                    nodes.push(MarkdownNode::EndQuote);
                                }
                                state = State::Root{spaces:0};
                            }
                            else if cursor.chars[0] == '>'{
                                for _ in 0..*blocks{
                                    nodes.push(MarkdownNode::EndQuote);
                                }
                                state = State::Root{spaces};
                            }
                            else if !last_is_space{
                                push_char(&mut nodes, &mut decoded, ' ');
                            }
                        }
                        Kind::Normal=>{
                            let last_is_space = cursor.last_char == ' ';
                            cursor.next();
                            while cursor.chars[0] == ' '{
                                cursor.next();
                            }
                            if cursor.chars[0] == '\n' || cursor.chars[0] == '\0'{
                                cursor.next();
                                state = State::Root{spaces:0};
                                nodes.push(MarkdownNode::EndNormal);
                            }
                            else if !last_is_space{
                                push_char(&mut nodes, &mut decoded, ' ');
                            }
                        }
                        Kind::List(depth)=>{
                            let last_is_space = cursor.last_char == ' ';
                            cursor.next();
                            let mut spaces = 0;
                            while cursor.chars[0] == ' '{
                                cursor.next();
                                spaces += 1;
                            }
                            if cursor.chars[0] == '\n' || cursor.chars[0] == '\0'{
                                cursor.next();
                                for _ in 0..*depth{
                                    nodes.push(MarkdownNode::EndListItem);
                                }
                                state = State::Root{spaces:0};
                            } 
                            else if (cursor.chars[0] == '+' || cursor.chars[0] == '*' || cursor.chars[0] == '-') && cursor.chars[1] == ' '{
                                for _ in 0..*depth{
                                    nodes.push(MarkdownNode::EndListItem);
                                }
                                state = State::Root{spaces};
                            }
                            else if cursor.chars[0].is_ascii_digit(){ // todo scan better
                                for _ in 0..*depth{
                                    nodes.push(MarkdownNode::EndListItem);
                                }
                                state = State::Root{spaces};
                            }
                            else if !last_is_space{
                                push_char(&mut nodes, &mut decoded, ' ');
                            }
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
                    // alright lets do it
                    let mut scan = cursor.clone();
                    scan.skip(1);
                    
                    let start = decoded.len();
                    // lets first patternmatch it
                    while scan.chars[0] != ']' && !scan.at_end(){
                        decoded.push(scan.chars[0]);
                        if scan.chars[0] == '\n' && scan.last_char == '\n'{
                            break; // double newline terminates scan
                        }
                        scan.next();
                    }
                    if scan.chars[0] == ']' && scan.chars[1] == '('{
                        scan.skip(2);
                        let url_start = decoded.len();
                        while scan.chars[0] != ')' && !scan.at_end(){
                            decoded.push(scan.chars[0]);
                            if scan.chars[0] == '\n' && scan.last_char == '\n'{
                                break; // double newline terminates scan
                            }
                            scan.next();
                        }
                        if scan.chars[0] == ')' {
                            scan.next();
                            nodes.push(MarkdownNode::Image{start, url_start, end:decoded.len()});
                            cursor = scan;
                            continue;
                        }
                    }
                    decoded.truncate(start);
                    decoded.push(cursor.chars[0]);
                    decoded.push(cursor.chars[1]);
                    // parse inline image
                    cursor.skip(2);
                }
                ['[',_,_]=>{ // possible named link
                    let mut scan = cursor.clone();
                    scan.skip(1);
                    let start = decoded.len();
                    // lets first patternmatch it
                    while scan.chars[0] != ']' && !scan.at_end(){
                        decoded.push(scan.chars[0]);
                        if scan.chars[0] == '\n' && scan.last_char == '\n'{
                            break; // double newline terminates scan
                        }
                        scan.next();
                        
                    }
                    if scan.chars[0] == ']' && scan.chars[1] == '('{
                        scan.skip(2);
                        let url_start = decoded.len();
                        while scan.chars[0] != ')' && !scan.at_end(){
                            decoded.push(scan.chars[0]);
                            if scan.chars[0] == '\n' && scan.last_char == '\n'{
                                break; // double newline terminates scan
                            }
                            scan.next();
                        }
                        // alright find last
                        if scan.chars[0] == ')' {
                            scan.next();
                            nodes.push(MarkdownNode::Link{start, url_start, end:decoded.len()});
                            cursor = scan;
                            continue;
                        }
                    }
                    decoded.truncate(start);
                    decoded.push(cursor.chars[0]);
                    cursor.next();
                }
                [' ',_,_]=>{
                    if cursor.last_char != ' '{
                        push_char(&mut nodes, &mut decoded, ' ');
                    }
                    cursor.next();
                }
                [x,_,_]=>{
                    push_char(&mut nodes, &mut decoded, x);
                    cursor.next();
                }
            }
            State::Root{spaces} => match cursor.chars{
                ['\0',_,_]=>{
                    break
                },
                [' ',_,_]=>{ // space counter
                    state = State::Root{spaces:*spaces + 1};
                    cursor.skip(1)
                }
                ['>',_,_]=>{
                    // alright lets parse and render the quotes
                    if *spaces>=4{ // its code
                        code_on_one_line(&mut nodes, &mut decoded, &mut cursor);
                        state = State::Root{spaces:0};
                    }
                    else{ // its a quote block, lets count all the >s and make quote blocks
                        let mut blocks = 0;
                        while cursor.chars[0] == ' ' || cursor.chars[0] == '>'{
                            if cursor.chars[0] == '>'{
                                blocks += 1;
                            }
                            cursor.next();
                        }
                        // ok so first we remove about as many begin
                        let mut removed = 0;
                        for _ in 0..blocks{
                            if let Some(MarkdownNode::EndQuote) = nodes.last(){
                                removed +=1;
                                nodes.pop();
                            }
                        }
                        for _ in 0..(blocks - removed){
                            nodes.push(MarkdownNode::BeginQuote);
                        }
                        push_optional_char(&mut nodes, &mut decoded, ' ');
                        // alright now we know how deep in the block stack we need to be
                        state = State::Inline{kind:Kind::Quote(blocks), bold:0, italic:0};
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
                        state = State::Inline{kind:Kind::Normal, bold:0, italic:0};
                    }
                    else {
                        cursor.next();
                        decoded.truncate(start);
                        nodes.push(MarkdownNode::BeginHead{level});
                        state = State::Inline{kind:Kind::Head, bold:0, italic:0};
                    }
                }
                ['`','`','`']=>{ // begins or ends blocks of code. 
                    cursor.skip(3);
                    nodes.push(MarkdownNode::BeginCode);
                    let start = decoded.len();
                    while cursor.chars != ['`','`','`'] && !cursor.at_end(){
                        if cursor.chars[0] == '\n' && start != decoded.len(){
                            nodes.push(MarkdownNode::NewLine);
                        }
                        else{
                            push_char(&mut nodes, &mut decoded, cursor.chars[0]);
                        }
                        cursor.skip(1);
                    }
                    if !cursor.at_end(){
                        cursor.skip(3);
                    }
                    // remove last newline
                    if let Some(MarkdownNode::NewLine) = nodes.last(){
                        nodes.pop();
                    }
                    nodes.push(MarkdownNode::EndCode);
                }
                ['-',' ',_] |
                ['*',' ',_] |
                ['+',' ',_] =>{ // possible list item
                    let depth = (*spaces >> 1) + 1;
                    let mut end_count = 0;
                    let mut iter = nodes.iter().rev();
                    while let Some(MarkdownNode::EndListItem) = iter.next(){
                        end_count += 1;
                    }
                    
                    if depth > end_count+1{
                        if *spaces>=4{ // its code
                            code_on_one_line(&mut nodes, &mut decoded, &mut cursor);
                            state = State::Root{spaces:0};
                        }
                        else{ // its normal 
                            nodes.push(MarkdownNode::BeginNormal);
                            state = State::Inline{kind:Kind::Normal, bold:0, italic:0};
                        }
                    }
                    else{
                        for i in 0..depth-1{
                            if i < end_count{
                                nodes.pop();
                            }
                        }
                        // we always push a begin list item on
                        nodes.push(MarkdownNode::BeginListItem{label:match cursor.chars[0]{
                            '-'=>MarkdownListLabel::Minus,
                            '*'=>MarkdownListLabel::Star,
                            '+'=>MarkdownListLabel::Plus,
                            _=>panic!()
                        }});
                        
                        state = State::Inline{kind:Kind::List(depth), bold:0, italic:0}
                    }
                    cursor.skip(2);
                    //push_optional_char(&mut nodes, &mut decoded, ' ');
                }
                ['\n',_,_]=>{ // skip it
                    cursor.skip(1);
                    state = State::Root{spaces:0};
                }
                [a,_b,_c]=>{
                    let mut is_list_digit = None;
                    if a.is_ascii_digit(){
                        let mut scan = cursor.clone();
                        let start = decoded.len();
                        while scan.chars[0].is_ascii_digit(){
                            decoded.push(scan.chars[0]);
                            scan.next();
                        }
                        if scan.chars[0] == '.' && scan.chars[1] == ' '{
                            decoded.push('.');
                            is_list_digit = Some((start, decoded.len()));
                            scan.skip(2);
                            cursor = scan;
                        }
                        else{
                            decoded.truncate(start);
                        }
                    }
                    if let Some((start, end)) = is_list_digit{
                        let depth = (*spaces >> 1) + 1;
                        let mut end_count = 0;
                        let mut iter = nodes.iter().rev();
                        while let Some(MarkdownNode::EndListItem) = iter.next(){
                            end_count += 1;
                        }
                                            
                        if depth > end_count+1{
                            if *spaces>=4{ // its code
                                code_on_one_line(&mut nodes, &mut decoded, &mut cursor);
                                state = State::Root{spaces:0};
                            }
                            else{ // its normal 
                                nodes.push(MarkdownNode::BeginNormal);
                                state = State::Inline{kind:Kind::Normal, bold:0, italic:0};
                            }
                        }
                        else{ 
                            for i in 0..depth-1{
                                if i < end_count{
                                    nodes.pop();
                                }
                            }
                            // we always push a begin list item on
                            nodes.push(MarkdownNode::BeginListItem{label:MarkdownListLabel::Number{
                                digit: decoded[start..end].parse::<usize>().unwrap_or(1),
                                start,
                                end
                            }});
                                                    
                            state = State::Inline{kind:Kind::List(depth), bold:0, italic:0}
                        }
                    }
                    else if *spaces>=4{ // its code
                        code_on_one_line(&mut nodes, &mut decoded, &mut cursor);
                        state = State::Root{spaces:0};
                    }
                    else{
                        nodes.push(MarkdownNode::BeginNormal);
                        state = State::Inline{kind:Kind::Normal, bold:0, italic:0};
                    }
                }
            }
        }
    }
    MarkdownDoc{
        nodes,
        decoded,
    }
}