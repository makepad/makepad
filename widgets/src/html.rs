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
    HtmlBase = {{Html}} {}
}

#[derive(Live, Widget)]
pub struct Html {
    #[redraw] #[live] draw_text: DrawText,
    #[live] body: Rc<String>,
    #[walk] walk: Walk,
    #[live] align: Align,
    #[live] padding: Padding,
    //margin: Margin,
    #[rust] html: Vec<HtmlNode>,
} 

// alright lets parse the HTML
impl LiveHook for Html{
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {
        self.html = parse_html(&self.body).unwrap();
        println!("{:?}", self.html);
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
        
}

// string/id wrappers

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

/*
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
}*/

// HTML Dom tree-flat vector
#[derive(Debug)]
pub enum HtmlNode{
    BeginElement(HtmlId),
    EndElement,
    Attribute(HtmlId, HtmlString),
    BoolAttribute(HtmlId),
    TextNode(HtmlString)
}

// parse it

pub fn parse_html(body:&Rc<String>)->Result<Vec<HtmlNode>,(String,usize)>{
    enum State{
        Text(usize),
        ElementName(usize),
        ElementClose(usize),
        ElementAttrs,
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
                        nodes.push(HtmlNode::TextNode(HtmlString::new(body, start, i)));
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
                    nodes.push(HtmlNode::BeginElement(HtmlId::new(&body, start, i)));
                    State::ElementAttrs
                }
                else if c == '/'{
                    nodes.push(HtmlNode::BeginElement(HtmlId::new(&body, start, i)));
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
                    // we could error on non matching tag here if we wanted, or just ignore it
                    nodes.push(HtmlNode::EndElement);
                    State::Text(i+1)
                }
                else{
                    State::ElementClose(start)
                }
            }
            State::ElementSelfClose=>{
                if c != '>'{
                    return Err(("Expected > after / self closed tag".into(),i))
                }
                nodes.push(HtmlNode::EndElement);
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
    Ok(nodes)
}
