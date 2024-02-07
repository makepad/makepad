use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*
    },
    std::rc::Rc,
    std::fmt
};

live_design!{
    HtmlBase = {{Html}} {
        // ok so we can use one drawtext
        // change to italic, change bold (SDF), strikethrough
        
    }
}

// this widget has a retained and an immediate mode api
#[derive(Live, Widget)]
pub struct Html {
    #[redraw] #[live] draw_text: DrawText,
    #[walk] walk: Walk,
    #[live] align: Align,
    #[live] padding: Padding,
    #[live] text: Rc<String>
} 

// alright lets parse the HTML
impl LiveHook for Html{
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {
        
    }
}

impl Widget for Html {

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk:Walk)->DrawStep{
        //self.draw_text.draw_walk(cx, walk.with_add_padding(self.padding), self.align, self.text.as_ref());
        DrawStep::done()
    }
    
    fn text(&self)->String{
        "".into()
        //self.text.as_ref().to_string()
    }
    
    fn set_text(&mut self, _v:&str){
        //self.text.as_mut_empty().push_str(v);
    }
}

impl Html{
    pub fn push_bold(&mut self){
    }
    
    pub fn pop_bold(&mut self){
    }
    
    pub fn push_italic(&mut self){
    }
    
    pub fn pop_italic(&mut self){
    }
    
    pub fn apply_widget(&mut self, _cx:&mut Cx, _id:&[LiveId;1], _apply:&[LiveNode]){
        
    }
    
    pub fn draw_text(&mut self, _cx:&mut Cx, _text:&str){
        
    }
    
    // parse it
    pub fn default_interpreter(&mut self, _cx:&mut Cx, _html:Vec<HtmlNode>){
        /*
        let node = html.walker();
        while let Some(node) = node.next(){
            match node.open_tag(){
                some_id!(a)=>{
                    let href = node.find_attr(id!(href));
                    self.apply_widget(cx, id!(MyWidget), live!{
                        label:(node.find_text().unwrap_or(""))
                    });
                }
                some_id!(strong)=>self.push_bold(),
                some_id!(em)=>self.push_italic(),
                _=>()
            }
            match node.close_tag(){
                some_id!(strong)=>self.pop_bold(),
                some_id!(em)=>self.pop_italic(),
                _=>()
            }
            if let Some(text) = node.text(){
                self.draw_text(cx, text);
            }
        }*/
    }
    
}


// Implementing a HTML renderer on top of the RichText widget

pub struct HtmlString(Rc<String>, usize, usize);

impl HtmlString{
    fn new(rc:&Rc<String>, start:usize, end:usize)->Self{
        Self(rc.clone(), start, end)
    }
    fn as_str(&self)->&str{
        &self.0[self.1..self.2]
    }
}

impl fmt::Debug for HtmlString{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/*
#[derive(Clone)]
pub struct HtmlId(pub HtmlString);
impl HtmlId{
    pub fn new(rc:&Rc<String>, start:usize, end:usize)->Self{
        Self(HtmlString(rc.clone(), start, end))
    }
    pub fn as_id(&self)->LiveId{
        LiveId::from_str(&self.0.0[self.0.1..self.0.2])
    }
}

impl fmt::Debug for HtmlId{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0.as_str())
    }
}
*/

#[derive(Clone)]
pub struct HtmlId(pub LiveId);
impl HtmlId{
    fn new(rc:&Rc<String>, start:usize, end:usize)->Self{
        Self(LiveId::from_str(&rc[start..end]))
    }
    pub fn as_id(&self)->LiveId{
        self.0
    }
}

impl fmt::Debug for HtmlId{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// HTML Dom tree-flat vector
#[derive(Debug)]
pub enum HtmlNode{
    OpenTag(HtmlId),
    CloseTag(HtmlId),
    Attribute(HtmlId, HtmlString),
    BoolAttribute(HtmlId),
    Text(HtmlString)
}



pub fn parse_html(body:&Rc<String>)->Result<Vec<HtmlNode>,(String,usize)>{
    enum State{
        Text(usize),
        ElementName(usize),
        ElementClose(usize),
        ElementAttrs,
        ElementCloseScanSpaces,
        ElementSelfClose,
        AttribName(usize),
        AttribValueEq(HtmlId),
        AttribValueStart(HtmlId),
        AttribValueSq(HtmlId, usize),
        AttribValueDq(HtmlId, usize),
        AttribValueBare(HtmlId, usize),
        CommentStartDash1,
        CommentStartDash2,
        CommentEndDash,
        CommentEnd,
        CommentBody
    }
    let mut nodes = Vec::new();
    let mut state = State::Text(0);
    for (i, c) in body.char_indices(){
        state = match state{
            State::Text(start)=>{ 
                if c == '<'{
                    if start != i{
                        nodes.push(HtmlNode::Text(HtmlString::new(body, start, i)));
                    }
                    State::ElementName(i+1)
                }
                else{
                    State::Text(start)
                }
            }
            State::ElementName(start)=>{
                if c == '/' && i == start{
                    State::ElementClose(i+1)
                }
                else if c == '!' && i == start{
                    State::CommentStartDash1
                }
                else if c.is_whitespace(){
                    if start == i{
                        return Err(("Found whitespace at beginning of tag".into(),start))
                    }
                    nodes.push(HtmlNode::OpenTag(HtmlId::new(&body, start, i)));
                    State::ElementAttrs
                }
                else if c == '/'{
                    nodes.push(HtmlNode::OpenTag(HtmlId::new(&body, start, i)));
                    State::ElementSelfClose
                }
                else if c == '>'{
                    State::Text(i+1)
                }
                else{
                    State::ElementName(start)
                }
            }
            State::ElementClose(start)=>{
                if c == '>'{
                    nodes.push(HtmlNode::CloseTag(HtmlId::new(&body, start, i)));
                    State::Text(i+1)
                }
                else if c.is_whitespace(){
                    nodes.push(HtmlNode::CloseTag(HtmlId::new(&body, start, i)));
                    State::ElementCloseScanSpaces
                }
                else{
                    State::ElementClose(start)
                }
            }
            State::ElementCloseScanSpaces=>{
                if c == '>'{
                    State::Text(i+1)
                }
                else if !c.is_whitespace(){
                    return Err(("Unexpected character after whitespace whilst looking for closing tag >".into(),i))
                }
                else{
                    State::ElementCloseScanSpaces
                }
            }
            State::ElementSelfClose=>{
                if c != '>'{
                    return Err(("Expected > after / self closed tag".into(),i))
                }
                // look backwards to the OpenTag
                let begin = nodes.iter().rev().find_map(|v| if let HtmlNode::OpenTag(tag) = v{Some(tag)}else{None}).unwrap();
                let tag = begin.clone();
                nodes.push(HtmlNode::CloseTag(tag));
                State::Text(i+1)
            }
            State::ElementAttrs=>{
                if c == '/'{
                    //nodes.push(HtmlNode::BeginElement(HtmlId::new(&body, start, i)));
                    State::ElementSelfClose
                }
                else if c == '>'{
                    State::Text(i+1)
                }
                else if !c.is_whitespace(){
                    State::AttribName(i)
                }
                else{
                    State::ElementAttrs
                }
            }
            State::AttribName(start)=>{
                if c.is_whitespace() {
                     State::AttribValueEq(HtmlId::new(body, start, i))
                }
                else if c == '='{
                     State::AttribValueStart(HtmlId::new(body, start, i))
                }
                else if c == '/'{
                    nodes.push(HtmlNode::BoolAttribute(HtmlId::new(body, start, i)));
                    State::ElementSelfClose
                }
                else if c == '>'{
                    nodes.push(HtmlNode::BoolAttribute(HtmlId::new(body, start, i)));
                     State::Text(i+1)
                }
                else{
                    State::AttribName(start)
                }
            }
            State::AttribValueEq(id)=>{
                if c == '/'{
                    nodes.push(HtmlNode::BoolAttribute(id));
                    State::ElementSelfClose
                }
                else if c == '>'{
                    nodes.push(HtmlNode::BoolAttribute(id));
                    State::Text(i+1)
                }
                else if c == '='{
                    State::AttribValueStart(id)
                }
                else if !c.is_whitespace(){
                    nodes.push(HtmlNode::BoolAttribute(id));
                    State::AttribName(i)
                }
                else{
                    State::AttribValueEq(id)
                }
            }
            State::AttribValueStart(id)=>{
                if c == '\"'{
                    // double quoted attrib
                    State::AttribValueDq(id, i+1)
                }
                else if c == '\''{
                    // single quoted attrib
                    State::AttribValueSq(id, i+1)
                }
                else if !c.is_whitespace(){
                    State::AttribValueBare(id, i)
                }
                else{
                    State::AttribValueStart(id)
                }
            }
            State::AttribValueSq(id, start)=>{
                if c == '\''{
                    nodes.push(HtmlNode::Attribute(id, HtmlString::new(&body, start, i)));
                    State::ElementAttrs
                }
                else{
                    State::AttribValueSq(id, start)
                }
            }
            State::AttribValueDq(id, start)=>{
                if c == '\"'{
                    nodes.push(HtmlNode::Attribute(id, HtmlString::new(&body, start, i)));
                    State::ElementAttrs
                }
                else{
                    State::AttribValueDq(id, start)
                }
            }
            State::AttribValueBare(id, start)=>{
                if c == '/'{
                    nodes.push(HtmlNode::Attribute(id, HtmlString::new(&body, start, i)));
                    State::ElementSelfClose
                }
                else if c == '>'{
                    nodes.push(HtmlNode::Attribute(id, HtmlString::new(&body, start, i)));
                    State::Text(i+1)
                }
                else if c.is_whitespace(){
                    nodes.push(HtmlNode::Attribute(id, HtmlString::new(&body, start, i)));
                    State::ElementAttrs
                }
                else{
                    State::AttribValueBare(id, start)
                }
            }
            State::CommentStartDash1=>{
                if c != '-'{
                    return Err(("Unexpected character looking for - after <!".into(),i));
                }
                State::CommentStartDash2
            }
            State::CommentStartDash2=>{
                if c != '-'{
                    return Err(("Unexpected character looking for - after <!-".into(),i));
                }
                State::CommentBody
            }
            State::CommentBody=>{
                if c == '-'{
                    State::CommentEndDash
                }
                else{
                    State::CommentBody
                }
            }
            State::CommentEndDash=>{
                if c == '-'{
                    State::CommentEnd
                }
                else{
                    State::CommentBody
                }
            },
            State::CommentEnd=>{
                if c == '>'{
                    State::Text(i+1)
                }
                else{
                    State::CommentBody
                }
            }
        }
    }
    if let State::Text(start) = state{
        if start != body.len(){
            nodes.push(HtmlNode::Text(HtmlString::new(body, start, body.len())));
        }
    }
    else{ // if we didnt end in text state something is wrong
        return Err(("HTML Parsing endstate is not HtmlNode::Text".into(),body.len()));
    }
    Ok(nodes)
}
