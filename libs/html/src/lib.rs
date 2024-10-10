use makepad_live_id::*;

#[derive(Debug)]
pub struct HtmlError{
    pub message:String,
    pub position:usize,
}

#[derive(Default, PartialEq)]
pub struct HtmlDoc{
    pub decoded: String,
    pub nodes: Vec<HtmlNode>,
}

#[derive(Debug, PartialEq)]
pub enum HtmlNode{
    OpenTag{lc:LiveId, nc:LiveId},
    CloseTag{lc:LiveId, nc:LiveId},
    Attribute{lc:LiveId, nc:LiveId, start:usize, end:usize},
    Text{start: usize, end:usize, all_ws:bool}
}
/*
/// A standalone owned copy of an HTML attribute.
#[derive(Debug, Clone)]
pub struct HtmlAttribute {
    /// The LiveID of this attribute's key converted to lowercase.
    pub lc: LiveId,
    /// The LiveID of this attribute's key in its original normal case.
    pub nc: LiveId,
    /// The value of this attribute.
    pub value: String,
}*/
 
pub struct HtmlWalker<'a>{
    decoded: &'a str,
    pub nodes: &'a [HtmlNode],
    pub index: usize,
}
 
impl<'a> HtmlWalker<'a>{
    pub fn index(&self)->usize{
        self.index
    }
 
    pub fn walk(&mut self){
        if self.index < self.nodes.len(){
            for i in self.index+1..self.nodes.len(){
                // we skip attributes
                if let HtmlNode::Attribute{..} = &self.nodes[i]{
                    
                }
                else{
                    self.index = i;
                    return;
                }
            }
            self.index = self.nodes.len();
        } 
    }
    
    pub fn jump_to_close(&mut self){
        if self.index < self.nodes.len(){
            let mut depth = 0;
            for i in self.index+1..self.nodes.len(){
                match &self.nodes[i]{
                    HtmlNode::OpenTag{..}=>{
                        depth +=1;
                    }
                    HtmlNode::CloseTag{..}=>{
                        if depth == 0{
                            self.index = i;
                            return;
                        }
                        depth -= 1;
                    }
                    _=>()
                }
            }
        } 
        self.index = self.nodes.len();
    }
    
    pub fn done(&self)->bool{
        self.index >= self.nodes.len()
    }
/*
    /// Iterates over and returns a list of all attributes for the current open HTML tag.
    pub fn collect_attributes(&self) -> Vec<HtmlAttribute> {
        let mut attrs = Vec::new();
        for node in &self.nodes[self.index ..] {
            match node {
                HtmlNode::Attribute { lc, nc, start, end } => {
                    attrs.push(HtmlAttribute {
                        lc: *lc,
                        nc: *nc,
                        value: String::from(&self.decoded[*start..*end]),
                    });
                }
                HtmlNode::CloseTag { .. } => break,
                _ => continue,
            }
        }
        attrs
    }
    */
    /// Returns the first attribute of the currently-opened Html tag
    /// whose key matches the given `flc` LiveId, which should be all lowercase.
    ///
    /// Matching is done after converting all attribute keys to lowercase.
    pub fn find_attr_lc(&self, flc:LiveId)->Option<&'a str>{
        for i in self.index..self.nodes.len(){
            match &self.nodes[i]{
                HtmlNode::CloseTag{..}=>{
                    return None
                }
                HtmlNode::Attribute{lc, nc:_, start, end} if *lc == flc=>{
                    return Some(&self.decoded[*start..*end])
                }
                _=>()
            }
        }
        None
    }
    
    pub fn while_attr_lc(&mut self)->Option<(LiveId, &'a str)>{
        if self.index<self.nodes.len(){
            match &self.nodes[self.index]{
                HtmlNode::Attribute{lc, nc:_, start, end}=>{
                    self.index += 1;
                    return Some((*lc, &self.decoded[*start..*end]))
                }
                _=>()
            }
        }
        None
    }
     
    /// Returns the first attribute of the currently-opened Html tag 
    /// whose key matches the given `fnc` LiveId, which is case-sensitive.
    ///
    /// Matching is done in a case-sensitive manner.
    pub fn find_attr_nc(&self, fnc:LiveId)->Option<&'a str>{
        for i in self.index..self.nodes.len(){
            match &self.nodes[i]{
                HtmlNode::CloseTag{..}=>{
                    return None
                }
                HtmlNode::Attribute{lc:_, nc, start, end} if *nc == fnc=>{
                    return Some(&self.decoded[*start..*end])
                }
                _=>()
            }
        }
        None
    }
    
    pub fn find_text(&self)->Option<&'a str>{
        for i in self.index..self.nodes.len(){
            match &self.nodes[i]{
                HtmlNode::CloseTag{..}=>{
                    return None
                }
                HtmlNode::Text{start, end,..} =>{
                    return Some(&self.decoded[*start..*end])
                }
                _=>()
            }
        }
        None
    }
    
    pub fn find_tag_text(&self, tag:LiveId)->Option<&'a str>{
        for i in self.index..self.nodes.len(){
            match &self.nodes[i]{
                HtmlNode::OpenTag{nc,..} if *nc == tag =>{
                    // the next one must be a text node
                    if let Some(HtmlNode::Text{start, end,..}) = self.nodes.get(i+1){
                        return Some(&self.decoded[*start..*end])
                    }
                }
                _=>()
            }
        }
        None
    }
        
    pub fn text(&self)->Option<&'a str>{
        match self.nodes.get(self.index){
            Some(HtmlNode::Text{start,end,..})=>{
                Some(&self.decoded[*start..*end])
            }
            _=> None
        }
    }
    
    pub fn text_is_all_ws(&self)->bool{
        match self.nodes.get(self.index){
            Some(HtmlNode::Text{all_ws,..})=>{
                *all_ws
            }
            _=> false
        }
    }
    
    pub fn open_tag_lc(&self)->Option<LiveId>{
        match self.nodes.get(self.index){
            Some(HtmlNode::OpenTag{lc,nc:_})=>{
                Some(*lc)
            }
            _=> None
        }
    }
    pub fn open_tag_nc(&self)->Option<LiveId>{
        match self.nodes.get(self.index){
            Some(HtmlNode::OpenTag{lc:_,nc})=>{
                Some(*nc)
            }
            _=> None
        }
    }
    pub fn open_tag(&self)->Option<(LiveId,LiveId)>{
        match self.nodes.get(self.index){
            Some(HtmlNode::OpenTag{lc,nc})=>{
                Some((*lc, *nc))
            }
            _=> None
        }
    }
    
    pub fn close_tag_lc(&self)->Option<LiveId>{
        match self.nodes.get(self.index){
            Some(HtmlNode::CloseTag{lc,nc:_})=>{
                Some(*lc)
            }
            _=> None
        }
    }
    pub fn close_tag_nc(&self)->Option<LiveId>{
        match self.nodes.get(self.index){
            Some(HtmlNode::CloseTag{lc:_,nc})=>{
                Some(*nc)
            }
            _=> None
        }
    }
    pub fn close_tag(&self)->Option<(LiveId,LiveId)>{
        match self.nodes.get(self.index){
            Some(HtmlNode::CloseTag{lc,nc})=>{
                Some((*lc, *nc))
            }
            _=> None
        }
    }
 }
 
 impl HtmlDoc{
     pub fn new_walker(&self)->HtmlWalker{
         HtmlWalker{
             decoded:&self.decoded,
             index: 0,
             nodes:&self.nodes,
         }
     }
     
     pub fn new_walker_with_index(&self, index: usize)->HtmlWalker{
         HtmlWalker{
             decoded:&self.decoded,
             index,
             nodes:&self.nodes,
         }
     }
 } 

 pub fn parse_html(body:&str, errors:  &mut Option<Vec<HtmlError>>, intern:InternLiveId)->HtmlDoc{
     enum State{
         Text(usize, usize, usize),
         ElementName(usize),
         ElementClose(usize),
         ElementAttrs,
         ElementCloseScanSpaces,
         ElementSelfClose,
         AttribName(usize),
         AttribValueEq(LiveId,LiveId),
         AttribValueStart(LiveId,LiveId),
         AttribValueSq(LiveId, LiveId, usize),
         AttribValueDq(LiveId, LiveId, usize),
         AttribValueBare(LiveId, LiveId, usize),
         CommentStartDash1,
         CommentStartDash2,
         CommentEndDash,
         CommentEnd,
         DocType,
         HeaderQuestion,
         HeaderAngle,
         CommentBody
     }
             
     fn process_entity(c:char, body:&str, in_entity:&mut Option<usize>, i:usize, decoded:&mut String, errors:&mut Option<Vec<HtmlError>>, last_non_whitespace:&mut usize){
         if c=='&'{
             if in_entity.is_some(){
                 if let Some(errors) = errors{errors.push(HtmlError{message:"Unexpected & inside entity".into(), position:i})};
             }
             *in_entity = Some(i+1);
         }
         else if let Some(start) = *in_entity{
             if c == ';'{
                 match match_entity(&body[start..i]){
                     Err(e)=>{
                         *in_entity = None;
                         if let Some(errors) = errors{errors.push(HtmlError{message:e, position:i})};
                         decoded.push_str(&body[start..i]);
                     }
                     Ok(entity)=>{
                         *in_entity = None;
                         decoded.push(std::char::from_u32(entity).unwrap());
                     }
                 }
             }
         }
         else{
             if c.is_whitespace() {
                decoded.push(c);                      
             }
             else{
                 decoded.push(c);
                 *last_non_whitespace = decoded.len();
             }
         }
     }
     
     let mut nodes = Vec::new();
     let mut state = State::Text(0, 0, 0);
     let mut decoded = String::new();
     let mut in_entity = None;
     
     for (i, c) in body.char_indices(){
         state = match state{
             State::DocType=>{
                 if c == '>'{
                     State::Text(i+1, decoded.len(), decoded.len())
                 }
                 else{
                     State::DocType
                 }
             }
             State::HeaderQuestion=>{
                 if c == '?'{
                    State::HeaderAngle
                 }
                 else{
                     State::HeaderQuestion
                 }              
             }
             State::HeaderAngle=>{
                 if c == '>'{
                     State::Text(i+1, decoded.len(), decoded.len())
                 }
                 else{
                     State::HeaderQuestion
                 }
             }
             State::Text(start, dec_start, last_non_whitespace)=>{ 
                 if c == '<'{
                    if let Some(start) = in_entity{
                        if let Some(errors) = errors{errors.push(HtmlError{message:"Unterminated entity".into(), position:start})};
                    }
                    nodes.push(HtmlNode::Text{start:dec_start, end:decoded.len(), all_ws:dec_start == last_non_whitespace});
                    State::ElementName(i+1)
                 }
                 else{
                     let mut last_non_whitespace = last_non_whitespace;
                     process_entity(c, &body, &mut in_entity, i, &mut decoded, errors, &mut last_non_whitespace);
                     State::Text(start, dec_start, last_non_whitespace)
                 }
             }
             State::ElementName(start)=>{
                 if c == '/' && i == start{
                     State::ElementClose(i+1)
                 }
                 else if c == '!' && i == start{
                     State::CommentStartDash1
                 }
                 else if c == '?'{
                     State::HeaderQuestion
                 }
                 else if c.is_whitespace(){
                     if start == i{
                          if let Some(errors) = errors{errors.push(HtmlError{message:"Found whitespace at beginning of tag".into(), position:i})};
                         State::Text(i+1, decoded.len(), decoded.len())
                     }
                     else{
                        nodes.push(HtmlNode::OpenTag{lc:LiveId::from_str_lc(&body[start..i]),nc:LiveId::from_str_with_intern(&body[start..i],intern)});
                        State::ElementAttrs
                    }
                }
                 else if c == '/'{
                     nodes.push(HtmlNode::OpenTag{lc:LiveId::from_str_lc(&body[start..i]),nc:LiveId::from_str_with_intern(&body[start..i], intern)});
                     State::ElementSelfClose
                 }
                 else if c == '>'{
                     nodes.push(HtmlNode::OpenTag{lc:LiveId::from_str_lc(&body[start..i]),nc:LiveId::from_str_with_intern(&body[start..i], intern)});
                     State::Text(i+1, decoded.len(), decoded.len())
                 }
                 else{
                     State::ElementName(start)
                 }
             }
             State::ElementClose(start)=>{
                 if c == '>'{
                     nodes.push(HtmlNode::CloseTag{lc:LiveId::from_str_lc(&body[start..i]),nc:LiveId::from_str_with_intern(&body[start..i], intern)});
                     State::Text(i+1, decoded.len(), decoded.len())
                 }
                 else if c.is_whitespace(){
                     nodes.push(HtmlNode::CloseTag{lc:LiveId::from_str_lc(&body[start..i]),nc:LiveId::from_str_with_intern(&body[start..i], intern)});
                     State::ElementCloseScanSpaces
                 }
                 else{
                     State::ElementClose(start)
                 }
             }
             State::ElementCloseScanSpaces=>{
                 if c == '>'{
                     State::Text(i+1, decoded.len(), decoded.len())
                 }
                 else if !c.is_whitespace(){
                      if let Some(errors) = errors{errors.push(HtmlError{message:"Unexpected character after whitespace whilst looking for closing tag >".into(), position:i})};
                     State::Text(i+1, decoded.len(), decoded.len())
                 }
                 else{
                     State::ElementCloseScanSpaces
                 }
             }
             State::ElementSelfClose=>{
                 if c != '>'{
                      if let Some(errors) = errors{errors.push(HtmlError{message:"Expected > after / self closed tag".into(), position:i})};
                 }
                 // look backwards to the OpenTag
                 let begin = nodes.iter().rev().find_map(|v| if let HtmlNode::OpenTag{lc,nc} = v{Some((lc,nc))}else{None}).unwrap();
                 nodes.push(HtmlNode::CloseTag{lc:*begin.0,nc:*begin.1});
                 State::Text(i+1, decoded.len(), decoded.len())
             }
             State::ElementAttrs=>{
                 if c == '/'{
                     //nodes.push(HtmlNode::BeginElement(HtmlId::new(&body, start, i)));
                     State::ElementSelfClose
                 }
                 else if c == '>'{
                     State::Text(i+1, decoded.len(), decoded.len())
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
                     State::AttribValueEq(LiveId::from_str_lc(&body[start..i]),LiveId::from_str_with_intern(&body[start..i], intern))
                 }
                 else if c == '='{
                     State::AttribValueStart(LiveId::from_str_lc(&body[start..i]),LiveId::from_str_with_intern(&body[start..i], intern))
                 }
                 else if c == '/'{
                     nodes.push(HtmlNode::Attribute{lc:LiveId::from_str_lc(&body[start..i]),nc:LiveId::from_str_with_intern(&body[start..i], intern),start:0,end:0});
                     State::ElementSelfClose
                 }
                 else if c == '>'{
                     nodes.push(HtmlNode::Attribute{lc:LiveId::from_str_lc(&body[start..i]),nc:LiveId::from_str_with_intern(&body[start..i], intern),start:0,end:0});
                     State::Text(i+1, decoded.len(), decoded.len())
                 }
                 else{
                     State::AttribName(start)
                 }
             }
             State::AttribValueEq(lc,nc)=>{
                 if c == '/'{
                     nodes.push(HtmlNode::Attribute{lc,nc,start:0,end:0});
                     State::ElementSelfClose
                 }
                 else if c == '>'{
                     nodes.push(HtmlNode::Attribute{lc,nc,start:0,end:0});
                     State::Text(i+1, decoded.len(), decoded.len())
                 }
                 else if c == '='{
                     State::AttribValueStart(lc,nc)
                 }
                 else if !c.is_whitespace(){
                     nodes.push(HtmlNode::Attribute{lc,nc,start:0,end:0});
                     State::AttribName(i)
                 }
                 else{
                     State::AttribValueEq(lc,nc)
                 }
             }
             State::AttribValueStart(lc,nc)=>{
                 if c == '\"'{
                     // double quoted attrib
                     State::AttribValueDq(lc,nc, decoded.len())
                 }
                 else if c == '\''{
                     // single quoted attrib 
                     State::AttribValueSq(lc,nc, decoded.len())
                 }
                 else if !c.is_whitespace(){
                     decoded.push(c);
                     State::AttribValueBare(lc,nc, decoded.len()-1)
                 }
                 else{
                     State::AttribValueStart(lc,nc)
                 }
             }
             State::AttribValueSq(lc,nc, start)=>{
                 if c == '\''{
                     if let Some(start) = in_entity{
                          if let Some(errors) = errors{errors.push(HtmlError{message:"Unterminated entity".into(), position:start})};
                     }
                     nodes.push(HtmlNode::Attribute{lc,nc, start, end:decoded.len()});
                     State::ElementAttrs
                 }
                 else{
                     process_entity(c, &body, &mut in_entity, i, &mut decoded, errors, &mut 0);
                     State::AttribValueSq(lc,nc, start)
                 }
             }
             State::AttribValueDq(lc,nc, start)=>{
                 if c == '\"'{
                     if let Some(start) = in_entity{
                          if let Some(errors) = errors{errors.push(HtmlError{message:"Unterminated entity".into(), position:start})};
                     }
                     nodes.push(HtmlNode::Attribute{lc,nc, start, end:decoded.len()});
                     State::ElementAttrs
                 }
                 else{
                     process_entity(c, &body, &mut in_entity, i, &mut decoded,  errors, &mut 0);
                     State::AttribValueDq(lc,nc, start)
                 }
             }
             State::AttribValueBare(lc,nc, start)=>{
                 if c == '/'{
                     nodes.push(HtmlNode::Attribute{lc,nc, start, end:decoded.len()});
                     State::ElementSelfClose
                 }
                 else if c == '>'{
                     nodes.push(HtmlNode::Attribute{lc,nc, start, end:decoded.len()});
                     State::Text(i+1, decoded.len(), decoded.len())
                 }
                 else if c.is_whitespace(){
                     nodes.push(HtmlNode::Attribute{lc,nc, start, end:decoded.len()});
                     State::ElementAttrs
                 }
                 else{
                     decoded.push(c);
                     State::AttribValueBare(lc,nc, start)
                 }
             }
             State::CommentStartDash1=>{
                 if c != '-'{
                     // we should scan for >
                     State::DocType
                     // if let Some(errors) = errors{errors.push(HtmlError{message:"Unexpected //character looking for - after <!".into(), position:i})};
                 }
                 else{
                    State::CommentStartDash2
                }
             }
             State::CommentStartDash2=>{
                 if c != '-'{
                      if let Some(errors) = errors{errors.push(HtmlError{message:"Unexpected character looking for - after <!-".into(), position:i})};
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
                     State::Text(i+1, decoded.len(), decoded.len())
                 }
                 else{
                     State::CommentBody
                 }
             }
         }
     }
     if let Some(start) = in_entity{
          if let Some(errors) = errors{errors.push(HtmlError{message:"Unterminated entity".into(), position:start})};
     }
     if let State::Text(_, dec_start, last_non_whitespace) = state{
        nodes.push(HtmlNode::Text{start:dec_start, end:decoded.len(), all_ws:dec_start == last_non_whitespace});
     }
     else{ // if we didnt end in text state something is wrong
          if let Some(errors) = errors{errors.push(HtmlError{message:"HTML Parsing endstate is not HtmlNode::Text".into(), position:body.len()})};
     }
     HtmlDoc{
         nodes,
         decoded,
     }
 }
 
 pub fn match_entity(what:&str)->Result<u32,String>{
     Ok(match what{
         "dollar"=>36,
         "DOLLAR"=>36,
         "cent"=>162,
         "CENT"=>162,
         "pound"=>163,
         "POUND"=>163,
         "curren"=>164,
         "CURREN"=>164,
         "yen"=>165,
         "YEN"=>165,
         "copy"=>169,
         "COPY"=>169,
         "reg"=>174,
         "REG"=>174,
         "trade"=>8482,
         "TRADE"=>8482,
         "commat"=>64,
         "COMMAT"=>64,
         "Copf"=>8450,
         "copf"=>8450,
         "COPF"=>8450,
         "incare"=>8453,
         "INCARE"=>8453,
         "gscr"=>8458,
         "GSCR"=>8458,
         "hamilt"=>8459,
         "HAMILT"=>8459,
         "Hfr"=>8460,
         "hfr"=>8460,
         "HFR"=>8460,
         "Hopf"=>8461,
         "hopf"=>8461,
         "HOPF"=>8461,
         "planckh"=>8462,
         "PLANCKH"=>8462,
         "planck"=>8463,
         "PLANCK"=>8463,
         "Iscr"=>8464,
         "iscr"=>8464,
         "ISCR"=>8464,
         "image"=>8465,
         "IMAGE"=>8465,
         "Lscr"=>8466,
         "lscr"=>8466,
         "LSCR"=>8466,
         "ell"=>8467,
         "ELL"=>8467,
         "Nopf"=>8469,
         "nopf"=>8469,
         "NOPF"=>8469,
         "numero"=>8470,
         "NUMERO"=>8470,
         "copysr"=>8471,
         "COPYSR"=>8471,
         "weierp"=>8472,
         "WEIERP"=>8472,
         "Popf"=>8473,
         "popf"=>8473,
         "POPF"=>8473,
         "Qopf"=>8474,
         "qopf"=>8474,
         "QOPF"=>8474,
         "Rscr"=>8475,
         "rscr"=>8475,
         "RSCR"=>8475,
         "real"=>8476,
         "REAL"=>8476,
         "Ropf"=>8477,
         "ropf"=>8477,
         "ROPF"=>8477,
         "rx"=>8478,
         "RX"=>8478,
         "Zopf"=>8484,
         "zopf"=>8484,
         "ZOPF"=>8484,
         "mho"=>8487,
         "MHO"=>8487,
         "Zfr"=>8488,
         "zfr"=>8488,
         "ZFR"=>8488,
         "iiota"=>8489,
         "IIOTA"=>8489,
         "bernou"=>8492,
         "BERNOU"=>8492,
         "Cfr"=>8493,
         "cfr"=>8493,
         "CFR"=>8493,
         "escr"=>8495,
         "ESCR"=>8495,
         "Escr"=>8496,
         "Fscr"=>8497,
         "fscr"=>8497,
         "FSCR"=>8497,
         "Mscr"=>8499,
         "mscr"=>8499,
         "MSCR"=>8499,
         "oscr"=>8500,
         "OSCR"=>8500,
         "alefsym"=>8501,
         "ALEFSYM"=>8501,
         "beth"=>8502,
         "BETH"=>8502,
         "gimel"=>8503,
         "GIMEL"=>8503,
         "daleth"=>8504,
         "DALETH"=>8504,
         "DD"=>8517,
         "dd"=>8517,
         "ee"=>8519,
         "EE"=>8519,
         "ii"=>8520,
         "II"=>8520,
         "starf"=>9733,
         "STARF"=>9733,
         "star"=>9734,
         "STAR"=>9734,
         "phone"=>9742,
         "PHONE"=>9742,
         "female"=>9792,
         "FEMALE"=>9792,
         "male"=>9794,
         "MALE"=>9794,
         "spades"=>9824,
         "SPADES"=>9824,
         "clubs"=>9827,
         "CLUBS"=>9827,
         "hearts"=>9829,
         "HEARTS"=>9829,
         "diams"=>9830,
         "DIAMS"=>9830,
         "loz"=>9674,
         "LOZ"=>9674,
         "sung"=>9834,
         "SUNG"=>9834,
         "flat"=>9837,
         "FLAT"=>9837,
         "natural"=>9838,
         "NATURAL"=>9838,
         "sharp"=>9839,
         "SHARP"=>9839,
         "check"=>10003,
         "CHECK"=>10003,
         "cross"=>10007,
         "CROSS"=>10007,
         "malt"=>10016,
         "MALT"=>10016,
         "sext"=>10038,
         "SEXT"=>10038,
         "VerticalSeparator"=>10072,
         "verticalseparator"=>10072,
         "VERTICALSEPARATOR"=>10072,
         "lbbrk"=>10098,
         "LBBRK"=>10098,
         "rbbrk"=>10099,
         "RBBRK"=>10099,
         "excl"=>33,
         "EXCL"=>33,
         "num"=>35,
         "NUM"=>35,
         "percnt"=>37,
         "PERCNT"=>37,
         "amp"=>38,
         "AMP"=>38,
         "lpar"=>40,
         "LPAR"=>40,
         "rpar"=>41,
         "RPAR"=>41,
         "ast"=>42,
         "AST"=>42,
         "comma"=>44,
         "COMMA"=>44,
         "period"=>46,
         "PERIOD"=>46,
         "sol"=>47,
         "SOL"=>47,
         "colon"=>58,
         "COLON"=>58,
         "semi"=>59,
         "SEMI"=>59,
         "quest"=>63,
         "QUEST"=>63,
         "lbrack"=>91,
         "LBRACK"=>91,
         "bsol"=>92,
         "BSOL"=>92,
         "rbrack"=>93,
         "RBRACK"=>93,
         "Hat"=>94,
         "hat"=>94,
         "HAT"=>94,
         "lowbar"=>95,
         "LOWBAR"=>95,
         "grave"=>96,
         "GRAVE"=>96,
         "lbrace"=>123,
         "LBRACE"=>123,
         "vert"=>124,
         "VERT"=>124,
         "rbrace"=>125,
         "RBRACE"=>125,
         "tilde"=>126,
         "TILDE"=>126,
         "circ"=>710,
         "CIRC"=>710,
         "nbsp"=>160,
         "NBSP"=>160,
         "ensp"=>8194,
         "ENSP"=>8194,
         "emsp"=>8195,
         "EMSP"=>8195,
         "thinsp"=>8201,
         "THINSP"=>8201,
         "zwnj"=>8204,
         "ZWNJ"=>8204,
         "zwj"=>8205,
         "ZWJ"=>8205,
         "lrm"=>8206,
         "LRM"=>8206,
         "rlm"=>8207,
         "RLM"=>8207,
         "iexcl"=>161,
         "IEXCL"=>161,
         "brvbar"=>166,
         "BRVBAR"=>166,
         "sect"=>167,
         "SECT"=>167,
         "uml"=>168,
         "UML"=>168,
         "ordf"=>170,
         "ORDF"=>170,
         "not"=>172,
         "NOT"=>172,
         "shy"=>173,
         "SHY"=>173,
         "macr"=>175,
         "MACR"=>175,
         "sup2"=>178,
         "SUP2"=>178,
         "sup3"=>179,
         "SUP3"=>179,
         "acute"=>180,
         "ACUTE"=>180,
         "micro"=>181,
         "MICRO"=>181,
         "para"=>182,
         "PARA"=>182,
         "middot"=>183,
         "MIDDOT"=>183,
         "cedil"=>184,
         "CEDIL"=>184,
         "sup1"=>185,
         "SUP1"=>185,
         "ordm"=>186,
         "ORDM"=>186,
         "iquest"=>191,
         "IQUEST"=>191,
         "hyphen"=>8208,
         "HYPHEN"=>8208,
         "ndash"=>8211,
         "NDASH"=>8211,
         "mdash"=>8212,
         "MDASH"=>8212,
         "horbar"=>8213,
         "HORBAR"=>8213,
         "Vert"=>8214,
         "dagger"=>8224,
         "DAGGER"=>8224,
         "Dagger"=>8225,
         "bull"=>8226,
         "BULL"=>8226,
         "nldr"=>8229,
         "NLDR"=>8229,
         "hellip"=>8230,
         "HELLIP"=>8230,
         ""=>8240,
         "pertenk"=>8241,
         "PERTENK"=>8241,
         "prime"=>8242,
         "PRIME"=>8242,
         "Prime"=>8243,
         "tprime"=>8244,
         "TPRIME"=>8244,
         "bprime"=>8245,
         "BPRIME"=>8245,
         "oline"=>8254,
         "OLINE"=>8254,
         "caret"=>8257,
         "CARET"=>8257,
         "hybull"=>8259,
         "HYBULL"=>8259,
         "frasl"=>8260,
         "FRASL"=>8260,
         "bsemi"=>8271,
         "BSEMI"=>8271,
         "qprime"=>8279,
         "QPRIME"=>8279,
         "quot"=>34,
         "QUOT"=>34,
         "apos"=>39,
         "APOS"=>39,
         "laquo"=>171,
         "LAQUO"=>171,
         "raquo"=>187,
         "RAQUO"=>187,
         "lsquo"=>8216,
         "LSQUO"=>8216,
         "rsquo"=>8217,
         "RSQUO"=>8217,
         "sbquo"=>8218,
         "SBQUO"=>8218,
         "ldquo"=>8220,
         "LDQUO"=>8220,
         "rdquo"=>8221,
         "RDQUO"=>8221,
         "bdquo"=>8222,
         "BDQUO"=>8222,
         "lsaquo"=>8249,
         "LSAQUO"=>8249,
         "rsaquo"=>8250,
         "RSAQUO"=>8250,
         "frac14"=>188,
         "FRAC14"=>188,
         "frac12"=>189,
         "FRAC12"=>189,
         "frac34"=>190,
         "FRAC34"=>190,
         "frac13"=>8531,
         "FRAC13"=>8531,
         "frac23"=>8532,
         "FRAC23"=>8532,
         "frac15"=>8533,
         "FRAC15"=>8533,
         "frac25"=>8534,
         "FRAC25"=>8534,
         "frac35"=>8535,
         "FRAC35"=>8535,
         "frac45"=>8536,
         "FRAC45"=>8536,
         "frac16"=>8537,
         "FRAC16"=>8537,
         "frac56"=>8538,
         "FRAC56"=>8538,
         "frac18"=>8539,
         "FRAC18"=>8539,
         "frac38"=>8540,
         "FRAC38"=>8540,
         "frac58"=>8541,
         "FRAC58"=>8541,
         "frac78"=>8542,
         "FRAC78"=>8542,
         "plus"=>43,
         "PLUS"=>43,
         "minus"=>8722,
         "MINUS"=>8722,
         "times"=>215,
         "TIMES"=>215,
         "divide"=>247,
         "DIVIDE"=>247,
         "equals"=>61,
         "EQUALS"=>61,
         "ne"=>8800,
         "NE"=>8800,
         "plusmn"=>177,
         "PLUSMN"=>177,
         "lt"=>60,
         "LT"=>60,
         "gt"=>62,
         "GT"=>62,
         "deg"=>176,
         "DEG"=>176,
         "fnof"=>402,
         "FNOF"=>402,
         "permil"=>137,
         "PERMIL"=>137,
         "forall"=>8704,
         "FORALL"=>8704,
         "comp"=>8705,
         "COMP"=>8705,
         "part"=>8706,
         "PART"=>8706,
         "exist"=>8707,
         "EXIST"=>8707,
         "nexist"=>8708,
         "NEXIST"=>8708,
         "empty"=>8709,
         "EMPTY"=>8709,
         "nabla"=>8711,
         "NABLA"=>8711,
         "isin"=>8712,
         "ISIN"=>8712,
         "notin"=>8713,
         "NOTIN"=>8713,
         "ni"=>8715,
         "NI"=>8715,
         "notni"=>8716,
         "NOTNI"=>8716,
         "prod"=>8719,
         "PROD"=>8719,
         "coprod"=>8720,
         "COPROD"=>8720,
         "sum"=>8721,
         "SUM"=>8721,
         "mnplus"=>8723,
         "MNPLUS"=>8723,
         "plusdo"=>8724,
         "PLUSDO"=>8724,
         "setminus"=>8726,
         "SETMINUS"=>8726,
         "lowast"=>8727,
         "LOWAST"=>8727,
         "compfn"=>8728,
         "COMPFN"=>8728,
         "radic"=>8730,
         "RADIC"=>8730,
         "prop"=>8733,
         "PROP"=>8733,
         "infin"=>8734,
         "INFIN"=>8734,
         "angrt"=>8735,
         "ANGRT"=>8735,
         "ang"=>8736,
         "ANG"=>8736,
         "angmsd"=>8737,
         "ANGMSD"=>8737,
         "angsph"=>8738,
         "ANGSPH"=>8738,
         "mid"=>8739,
         "MID"=>8739,
         "nmid"=>8740,
         "NMID"=>8740,
         "parallel"=>8741,
         "PARALLEL"=>8741,
         "npar"=>8742,
         "NPAR"=>8742,
         "and"=>8743,
         "AND"=>8743,
         "or"=>8744,
         "OR"=>8744,
         "cap"=>8745,
         "CAP"=>8745,
         "cup"=>8746,
         "CUP"=>8746,
         "int"=>8747,
         "INT"=>8747,
         "Int"=>8748,
         "iiint"=>8749,
         "IIINT"=>8749,
         "conint"=>8750,
         "CONINT"=>8750,
         "Conint"=>8751,
         "Cconint"=>8752,
         "cconint"=>8752,
         "CCONINT"=>8752,
         "cwint"=>8753,
         "CWINT"=>8753,
         "cwconint"=>8754,
         "CWCONINT"=>8754,
         "awconint"=>8755,
         "AWCONINT"=>8755,
         "there4"=>8756,
         "THERE4"=>8756,
         "because"=>8757,
         "BECAUSE"=>8757,
         "ratio"=>8758,
         "RATIO"=>8758,
         "Colon"=>8759,
         "minusd"=>8760,
         "MINUSD"=>8760,
         "mDDot"=>8762,
         "mddot"=>8762,
         "MDDOT"=>8762,
         "homtht"=>8763,
         "HOMTHT"=>8763,
         "sim"=>8764,
         "SIM"=>8764,
         "bsim"=>8765,
         "BSIM"=>8765,
         "ac"=>8766,
         "AC"=>8766,
         "acd"=>8767,
         "ACD"=>8767,
         "wreath"=>8768,
         "WREATH"=>8768,
         "nsim"=>8769,
         "NSIM"=>8769,
         "esim"=>8770,
         "ESIM"=>8770,
         "sime"=>8771,
         "SIME"=>8771,
         "nsime"=>8772,
         "NSIME"=>8772,
         "cong"=>8773,
         "CONG"=>8773,
         "simne"=>8774,
         "SIMNE"=>8774,
         "ncong"=>8775,
         "NCONG"=>8775,
         "asymp"=>8776,
         "ASYMP"=>8776,
         "nap"=>8777,
         "NAP"=>8777,
         "approxeq"=>8778,
         "APPROXEQ"=>8778,
         "apid"=>8779,
         "APID"=>8779,
         "bcong"=>8780,
         "BCONG"=>8780,
         "asympeq"=>8781,
         "ASYMPEQ"=>8781,
         "bump"=>8782,
         "BUMP"=>8782,
         "bumpe"=>8783,
         "BUMPE"=>8783,
         "esdot"=>8784,
         "ESDOT"=>8784,
         "eDot"=>8785,
         "edot"=>8785,
         "EDOT"=>8785,
         "efDot"=>8786,
         "efdot"=>8786,
         "EFDOT"=>8786,
         "erDot"=>8787,
         "erdot"=>8787,
         "ERDOT"=>8787,
         "colone"=>8788,
         "COLONE"=>8788,
         "ecolon"=>8789,
         "ECOLON"=>8789,
         "ecir"=>8790,
         "ECIR"=>8790,
         "cire"=>8791,
         "CIRE"=>8791,
         "wedgeq"=>8793,
         "WEDGEQ"=>8793,
         "veeeq"=>8794,
         "VEEEQ"=>8794,
         "trie"=>8796,
         "TRIE"=>8796,
         "equest"=>8799,
         "EQUEST"=>8799,
         "equiv"=>8801,
         "EQUIV"=>8801,
         "nequiv"=>8802,
         "NEQUIV"=>8802,
         "le"=>8804,
         "LE"=>8804,
         "ge"=>8805,
         "GE"=>8805,
         "lE"=>8806,
         "gE"=>8807,
         "lnE"=>8808,
         "lne"=>8808,
         "LNE"=>8808,
         "gnE"=>8809,
         "gne"=>8809,
         "GNE"=>8809,
         "Lt"=>8810,
         "Gt"=>8811,
         "between"=>8812,
         "BETWEEN"=>8812,
         "NotCupCap"=>8813,
         "notcupcap"=>8813,
         "NOTCUPCAP"=>8813,
         "nlt"=>8814,
         "NLT"=>8814,
         "ngt"=>8815,
         "NGT"=>8815,
         "nle"=>8816,
         "NLE"=>8816,
         "nge"=>8817,
         "NGE"=>8817,
         "lsim"=>8818,
         "LSIM"=>8818,
         "gsim"=>8819,
         "GSIM"=>8819,
         "nlsim"=>8820,
         "NLSIM"=>8820,
         "ngsim"=>8821,
         "NGSIM"=>8821,
         "lg"=>8822,
         "LG"=>8822,
         "gl"=>8823,
         "GL"=>8823,
         "ntlg"=>8824,
         "NTLG"=>8824,
         "ntgl"=>8825,
         "NTGL"=>8825,
         "pr"=>8826,
         "PR"=>8826,
         "sc"=>8827,
         "SC"=>8827,
         "prcue"=>8828,
         "PRCUE"=>8828,
         "sccue"=>8829,
         "SCCUE"=>8829,
         "prsim"=>8830,
         "PRSIM"=>8830,
         "scsim"=>8831,
         "SCSIM"=>8831,
         "npr"=>8832,
         "NPR"=>8832,
         "nsc"=>8833,
         "NSC"=>8833,
         "sub"=>8834,
         "SUB"=>8834,
         "sup"=>8835,
         "SUP"=>8835,
         "nsub"=>8836,
         "NSUB"=>8836,
         "nsup"=>8837,
         "NSUP"=>8837,
         "sube"=>8838,
         "SUBE"=>8838,
         "supe"=>8839,
         "SUPE"=>8839,
         "nsube"=>8840,
         "NSUBE"=>8840,
         "nsupe"=>8841,
         "NSUPE"=>8841,
         "subne"=>8842,
         "SUBNE"=>8842,
         "supne"=>8843,
         "SUPNE"=>8843,
         "cupdot"=>8845,
         "CUPDOT"=>8845,
         "uplus"=>8846,
         "UPLUS"=>8846,
         "sqsub"=>8847,
         "SQSUB"=>8847,
         "sqsup"=>8848,
         "SQSUP"=>8848,
         "sqsube"=>8849,
         "SQSUBE"=>8849,
         "sqsupe"=>8850,
         "SQSUPE"=>8850,
         "sqcap"=>8851,
         "SQCAP"=>8851,
         "sqcup"=>8852,
         "SQCUP"=>8852,
         "oplus"=>8853,
         "OPLUS"=>8853,
         "ominus"=>8854,
         "OMINUS"=>8854,
         "otimes"=>8855,
         "OTIMES"=>8855,
         "osol"=>8856,
         "OSOL"=>8856,
         "odot"=>8857,
         "ODOT"=>8857,
         "ocir"=>8858,
         "OCIR"=>8858,
         "oast"=>8859,
         "OAST"=>8859,
         "odash"=>8861,
         "ODASH"=>8861,
         "plusb"=>8862,
         "PLUSB"=>8862,
         "minusb"=>8863,
         "MINUSB"=>8863,
         "timesb"=>8864,
         "TIMESB"=>8864,
         "sdotb"=>8865,
         "SDOTB"=>8865,
         "vdash"=>8866,
         "VDASH"=>8866,
         "dashv"=>8867,
         "DASHV"=>8867,
         "top"=>8868,
         "TOP"=>8868,
         "perp"=>8869,
         "PERP"=>8869,
         "models"=>8871,
         "MODELS"=>8871,
         "vDash"=>8872,
         "Vdash"=>8873,
         "Vvdash"=>8874,
         "vvdash"=>8874,
         "VVDASH"=>8874,
         "VDash"=>8875,
         "nvdash"=>8876,
         "NVDASH"=>8876,
         "nvDash"=>8877,
         "nVdash"=>8878,
         "nVDash"=>8879,
         "prurel"=>8880,
         "PRUREL"=>8880,
         "vltri"=>8882,
         "VLTRI"=>8882,
         "vrtri"=>8883,
         "VRTRI"=>8883,
         "ltrie"=>8884,
         "LTRIE"=>8884,
         "rtrie"=>8885,
         "RTRIE"=>8885,
         "origof"=>8886,
         "ORIGOF"=>8886,
         "imof"=>8887,
         "IMOF"=>8887,
         "mumap"=>8888,
         "MUMAP"=>8888,
         "hercon"=>8889,
         "HERCON"=>8889,
         "intcal"=>8890,
         "INTCAL"=>8890,
         "veebar"=>8891,
         "VEEBAR"=>8891,
         "barvee"=>8893,
         "BARVEE"=>8893,
         "angrtvb"=>8894,
         "ANGRTVB"=>8894,
         "lrtri"=>8895,
         "LRTRI"=>8895,
         "xwedge"=>8896,
         "XWEDGE"=>8896,
         "xvee"=>8897,
         "XVEE"=>8897,
         "xcap"=>8898,
         "XCAP"=>8898,
         "xcup"=>8899,
         "XCUP"=>8899,
         "diamond"=>8900,
         "DIAMOND"=>8900,
         "sdot"=>8901,
         "SDOT"=>8901,
         "Star"=>8902,
         "divonx"=>8903,
         "DIVONX"=>8903,
         "bowtie"=>8904,
         "BOWTIE"=>8904,
         "ltimes"=>8905,
         "LTIMES"=>8905,
         "rtimes"=>8906,
         "RTIMES"=>8906,
         "lthree"=>8907,
         "LTHREE"=>8907,
         "rthree"=>8908,
         "RTHREE"=>8908,
         "bsime"=>8909,
         "BSIME"=>8909,
         "cuvee"=>8910,
         "CUVEE"=>8910,
         "cuwed"=>8911,
         "CUWED"=>8911,
         "Sub"=>8912,
         "Sup"=>8913,
         "Cap"=>8914,
         "Cup"=>8915,
         "fork"=>8916,
         "FORK"=>8916,
         "epar"=>8917,
         "EPAR"=>8917,
         "ltdot"=>8918,
         "LTDOT"=>8918,
         "gtdot"=>8919,
         "GTDOT"=>8919,
         "Ll"=>8920,
         "ll"=>8920,
         "LL"=>8920,
         "Gg"=>8921,
         "gg"=>8921,
         "GG"=>8921,
         "leg"=>8922,
         "LEG"=>8922,
         "gel"=>8923,
         "GEL"=>8923,
         "cuepr"=>8926,
         "CUEPR"=>8926,
         "cuesc"=>8927,
         "CUESC"=>8927,
         "nprcue"=>8928,
         "NPRCUE"=>8928,
         "nsccue"=>8929,
         "NSCCUE"=>8929,
         "nsqsube"=>8930,
         "NSQSUBE"=>8930,
         "nsqsupe"=>8931,
         "NSQSUPE"=>8931,
         "lnsim"=>8934,
         "LNSIM"=>8934,
         "gnsim"=>8935,
         "GNSIM"=>8935,
         "prnsim"=>8936,
         "PRNSIM"=>8936,
         "scnsim"=>8937,
         "SCNSIM"=>8937,
         "nltri"=>8938,
         "NLTRI"=>8938,
         "nrtri"=>8939,
         "NRTRI"=>8939,
         "nltrie"=>8940,
         "NLTRIE"=>8940,
         "nrtrie"=>8941,
         "NRTRIE"=>8941,
         "vellip"=>8942,
         "VELLIP"=>8942,
         "ctdot"=>8943,
         "CTDOT"=>8943,
         "utdot"=>8944,
         "UTDOT"=>8944,
         "dtdot"=>8945,
         "DTDOT"=>8945,
         "disin"=>8946,
         "DISIN"=>8946,
         "isinsv"=>8947,
         "ISINSV"=>8947,
         "isins"=>8948,
         "ISINS"=>8948,
         "isindot"=>8949,
         "ISINDOT"=>8949,
         "notinvc"=>8950,
         "NOTINVC"=>8950,
         "notinvb"=>8951,
         "NOTINVB"=>8951,
         "isinE"=>8953,
         "isine"=>8953,
         "ISINE"=>8953,
         "nisd"=>8954,
         "NISD"=>8954,
         "xnis"=>8955,
         "XNIS"=>8955,
         "nis"=>8956,
         "NIS"=>8956,
         "notnivc"=>8957,
         "NOTNIVC"=>8957,
         "notnivb"=>8958,
         "NOTNIVB"=>8958,
         "lceil"=>8968,
         "LCEIL"=>8968,
         "rceil"=>8969,
         "RCEIL"=>8969,
         "lfloor"=>8970,
         "LFLOOR"=>8970,
         "rfloor"=>8971,
         "RFLOOR"=>8971,
         "lang"=>9001,
         "LANG"=>9001,
         "rang"=>9002,
         "RANG"=>9002,
         "Alpha"=>913,
         "alpha"=>913,
         "ALPHA"=>913,
         "Beta"=>914,
         "beta"=>914,
         "BETA"=>914,
         "Gamma"=>915,
         "gamma"=>915,
         "GAMMA"=>915,
         "Delta"=>916,
         "delta"=>916,
         "DELTA"=>916,
         "Epsilon"=>917,
         "epsilon"=>917,
         "EPSILON"=>917,
         "Zeta"=>918,
         "zeta"=>918,
         "ZETA"=>918,
         "Eta"=>919,
         "eta"=>919,
         "ETA"=>919,
         "Theta"=>920,
         "theta"=>920,
         "THETA"=>920,
         "Iota"=>921,
         "iota"=>921,
         "IOTA"=>921,
         "Kappa"=>922,
         "kappa"=>922,
         "KAPPA"=>922,
         "Lambda"=>923,
         "lambda"=>923,
         "LAMBDA"=>923,
         "Mu"=>924,
         "mu"=>924,
         "MU"=>924,
         "Nu"=>925,
         "nu"=>925,
         "NU"=>925,
         "Xi"=>926,
         "xi"=>926,
         "XI"=>926,
         "Omicron"=>927,
         "omicron"=>927,
         "OMICRON"=>927,
         "Pi"=>928,
         "pi"=>928,
         "PI"=>928,
         "Rho"=>929,
         "rho"=>929,
         "RHO"=>929,
         "Sigma"=>931,
         "sigma"=>931,
         "SIGMA"=>931,
         "Tau"=>932,
         "tau"=>932,
         "TAU"=>932,
         "Upsilon"=>933,
         "upsilon"=>933,
         "UPSILON"=>933,
         "Phi"=>934,
         "phi"=>934,
         "PHI"=>934,
         "Chi"=>935,
         "chi"=>935,
         "CHI"=>935,
         "Psi"=>936,
         "psi"=>936,
         "PSI"=>936,
         "Omega"=>937,
         "omega"=>937,
         "OMEGA"=>937,
         "sigmaf"=>962,
         "SIGMAF"=>962,
         "thetasym"=>977,
         "THETASYM"=>977,
         "upsih"=>978,
         "UPSIH"=>978,
         "piv"=>982,
         "PIV"=>982,
         "Agrave"=>192,
         "agrave"=>192,
         "AGRAVE"=>192,
         "Aacute"=>193,
         "aacute"=>193,
         "AACUTE"=>193,
         "Acirc"=>194,
         "acirc"=>194,
         "ACIRC"=>194,
         "Atilde"=>195,
         "atilde"=>195,
         "ATILDE"=>195,
         "Auml"=>196,
         "auml"=>196,
         "AUML"=>196,
         "Aring"=>197,
         "aring"=>197,
         "ARING"=>197,
         "AElig"=>198,
         "aelig"=>198,
         "AELIG"=>198,
         "Ccedil"=>199,
         "ccedil"=>199,
         "CCEDIL"=>199,
         "Egrave"=>200,
         "egrave"=>200,
         "EGRAVE"=>200,
         "Eacute"=>201,
         "eacute"=>201,
         "EACUTE"=>201,
         "Ecirc"=>202,
         "ecirc"=>202,
         "ECIRC"=>202,
         "Euml"=>203,
         "euml"=>203,
         "EUML"=>203,
         "Lgrave"=>204,
         "lgrave"=>204,
         "LGRAVE"=>204,
         "Lacute"=>313,
         "lacute"=>313,
         "LACUTE"=>313,
         "Lcirc"=>206,
         "lcirc"=>206,
         "LCIRC"=>206,
         "Luml"=>207,
         "luml"=>207,
         "LUML"=>207,
         "ETH"=>208,
         "eth"=>208,
         "Ntilde"=>209,
         "ntilde"=>209,
         "NTILDE"=>209,
         "Ograve"=>210,
         "ograve"=>210,
         "OGRAVE"=>210,
         "Oacute"=>211,
         "oacute"=>211,
         "OACUTE"=>211,
         "Ocirc"=>212,
         "ocirc"=>212,
         "OCIRC"=>212,
         "Otilde"=>213,
         "otilde"=>213,
         "OTILDE"=>213,
         "Ouml"=>214,
         "ouml"=>214,
         "OUML"=>214,
         "Oslash"=>216,
         "oslash"=>216,
         "OSLASH"=>216,
         "Ugrave"=>217,
         "ugrave"=>217,
         "UGRAVE"=>217,
         "Uacute"=>218,
         "uacute"=>218,
         "UACUTE"=>218,
         "Ucirc"=>219,
         "ucirc"=>219,
         "UCIRC"=>219,
         "Uuml"=>220,
         "uuml"=>220,
         "UUML"=>220,
         "Yacute"=>221,
         "yacute"=>221,
         "YACUTE"=>221,
         "THORN"=>222,
         "thorn"=>222,
         "szlig"=>223,
         "SZLIG"=>223,
         "igrave"=>236,
         "IGRAVE"=>236,
         "iacute"=>237,
         "IACUTE"=>237,
         "icirc"=>238,
         "ICIRC"=>238,
         "iuml"=>239,
         "IUML"=>239,
         "yuml"=>255,
         "YUML"=>255,
         "Amacr"=>256,
         "amacr"=>256,
         "AMACR"=>256,
         "Abreve"=>258,
         "abreve"=>258,
         "ABREVE"=>258,
         "Aogon"=>260,
         "aogon"=>260,
         "AOGON"=>260,
         "Cacute"=>262,
         "cacute"=>262,
         "CACUTE"=>262,
         "Ccirc"=>264,
         "ccirc"=>264,
         "CCIRC"=>264,
         "Cdot"=>266,
         "cdot"=>266,
         "CDOT"=>266,
         "Ccaron"=>268,
         "ccaron"=>268,
         "CCARON"=>268,
         "Dcaron"=>270,
         "dcaron"=>270,
         "DCARON"=>270,
         "Dstrok"=>272,
         "dstrok"=>272,
         "DSTROK"=>272,
         "Emacr"=>274,
         "emacr"=>274,
         "EMACR"=>274,
         "Edot"=>278,
         "Eogon"=>280,
         "eogon"=>280,
         "EOGON"=>280,
         "Ecaron"=>282,
         "ecaron"=>282,
         "ECARON"=>282,
         "Gcirc"=>284,
         "gcirc"=>284,
         "GCIRC"=>284,
         "Gbreve"=>286,
         "gbreve"=>286,
         "GBREVE"=>286,
         "Gdot"=>288,
         "gdot"=>288,
         "GDOT"=>288,
         "Gcedil"=>290,
         "gcedil"=>290,
         "GCEDIL"=>290,
         "Hcirc"=>292,
         "hcirc"=>292,
         "HCIRC"=>292,
         "Hstrok"=>294,
         "hstrok"=>294,
         "HSTROK"=>294,
         "Itilde"=>296,
         "itilde"=>296,
         "ITILDE"=>296,
         "Imacr"=>298,
         "imacr"=>298,
         "IMACR"=>298,
         "Iogon"=>302,
         "iogon"=>302,
         "IOGON"=>302,
         "Idot"=>304,
         "idot"=>304,
         "IDOT"=>304,
         "imath"=>305,
         "IMATH"=>305,
         "IJlig"=>306,
         "ijlig"=>306,
         "IJLIG"=>306,
         "Jcirc"=>308,
         "jcirc"=>308,
         "JCIRC"=>308,
         "Kcedil"=>310,
         "kcedil"=>310,
         "KCEDIL"=>310,
         "kgreen"=>312,
         "KGREEN"=>312,
         "Lcedil"=>315,
         "lcedil"=>315,
         "LCEDIL"=>315,
         "Lcaron"=>317,
         "lcaron"=>317,
         "LCARON"=>317,
         "Lmidot"=>319,
         "lmidot"=>319,
         "LMIDOT"=>319,
         "Lstrok"=>321,
         "lstrok"=>321,
         "LSTROK"=>321,
         "Nacute"=>323,
         "nacute"=>323,
         "NACUTE"=>323,
         "Ncedil"=>325,
         "ncedil"=>325,
         "NCEDIL"=>325,
         "Ncaron"=>327,
         "ncaron"=>327,
         "NCARON"=>327,
         "napos"=>329,
         "NAPOS"=>329,
         "ENG"=>330,
         "eng"=>330,
         "Omacr"=>332,
         "omacr"=>332,
         "OMACR"=>332,
         "Odblac"=>336,
         "odblac"=>336,
         "ODBLAC"=>336,
         "OElig"=>338,
         "oelig"=>338,
         "OELIG"=>338,
         "Racute"=>340,
         "racute"=>340,
         "RACUTE"=>340,
         "Rcedil"=>342,
         "rcedil"=>342,
         "RCEDIL"=>342,
         "Rcaron"=>344,
         "rcaron"=>344,
         "RCARON"=>344,
         "Sacute"=>346,
         "sacute"=>346,
         "SACUTE"=>346,
         "Scirc"=>348,
         "scirc"=>348,
         "SCIRC"=>348,
         "Scedil"=>350,
         "scedil"=>350,
         "SCEDIL"=>350,
         "Scaron"=>352,
         "scaron"=>352,
         "SCARON"=>352,
         "Tcedil"=>354,
         "tcedil"=>354,
         "TCEDIL"=>354,
         "Tcaron"=>356,
         "tcaron"=>356,
         "TCARON"=>356,
         "Tstrok"=>358,
         "tstrok"=>358,
         "TSTROK"=>358,
         "Utilde"=>360,
         "utilde"=>360,
         "UTILDE"=>360,
         "Umacr"=>362,
         "umacr"=>362,
         "UMACR"=>362,
         "Ubreve"=>364,
         "ubreve"=>364,
         "UBREVE"=>364,
         "Uring"=>366,
         "uring"=>366,
         "URING"=>366,
         "Udblac"=>368,
         "udblac"=>368,
         "UDBLAC"=>368,
         "Uogon"=>370,
         "uogon"=>370,
         "UOGON"=>370,
         "Wcirc"=>372,
         "wcirc"=>372,
         "WCIRC"=>372,
         "Ycirc"=>374,
         "ycirc"=>374,
         "YCIRC"=>374,
         "Yuml"=>376,
         "Zacute"=>377,
         "zacute"=>377,
         "ZACUTE"=>377,
         "Zdot"=>379,
         "zdot"=>379,
         "ZDOT"=>379,
         "Zcaron"=>381,
         "zcaron"=>381,
         "ZCARON"=>381,
         "DownBreve"=>785,
         "downbreve"=>785,
         "DOWNBREVE"=>785,
         "olarr"=>8634,
         "OLARR"=>8634,
         "orarr"=>8635,
         "ORARR"=>8635,
         "lharu"=>8636,
         "LHARU"=>8636,
         "lhard"=>8637,
         "LHARD"=>8637,
         "uharr"=>8638,
         "UHARR"=>8638,
         "uharl"=>8639,
         "UHARL"=>8639,
         "rharu"=>8640,
         "RHARU"=>8640,
         "rhard"=>8641,
         "RHARD"=>8641,
         "dharr"=>8642,
         "DHARR"=>8642,
         "dharl"=>8643,
         "DHARL"=>8643,
         "rlarr"=>8644,
         "RLARR"=>8644,
         "udarr"=>8645,
         "UDARR"=>8645,
         "lrarr"=>8646,
         "LRARR"=>8646,
         "llarr"=>8647,
         "LLARR"=>8647,
         "uuarr"=>8648,
         "UUARR"=>8648,
         "rrarr"=>8649,
         "RRARR"=>8649,
         "ddarr"=>8650,
         "DDARR"=>8650,
         "lrhar"=>8651,
         "LRHAR"=>8651,
         "rlhar"=>8652,
         "RLHAR"=>8652,
         "nlArr"=>8653,
         "nlarr"=>8653,
         "NLARR"=>8653,
         "nhArr"=>8654,
         "nharr"=>8654,
         "NHARR"=>8654,
         "nrArr"=>8655,
         "nrarr"=>8655,
         "NRARR"=>8655,
         "lArr"=>8656,
         "larr"=>8656,
         "LARR"=>8656,
         "uArr"=>8657,
         "uarr"=>8657,
         "UARR"=>8657,
         "rArr"=>8658,
         "rarr"=>8658,
         "RARR"=>8658,
         "dArr"=>8659,
         "darr"=>8659,
         "DARR"=>8659,
         "hArr"=>8660,
         "harr"=>8660,
         "HARR"=>8660,
         "vArr"=>8661,
         "varr"=>8661,
         "VARR"=>8661,
         "nwArr"=>8662,
         "nwarr"=>8662,
         "NWARR"=>8662,
         "neArr"=>8663,
         "nearr"=>8663,
         "NEARR"=>8663,
         "seArr"=>8664,
         "searr"=>8664,
         "SEARR"=>8664,
         "swArr"=>8665,
         "swarr"=>8665,
         "SWARR"=>8665,
         "lAarr"=>8666,
         "laarr"=>8666,
         "LAARR"=>8666,
         "rAarr"=>8667,
         "raarr"=>8667,
         "RAARR"=>8667,
         "ziglarr"=>8668,
         "ZIGLARR"=>8668,
         "zigrarr"=>8669,
         "ZIGRARR"=>8669,
         "larrb"=>8676,
         "LARRB"=>8676,
         "rarrb"=>8677,
         "RARRB"=>8677,
         "duarr"=>8693,
         "DUARR"=>8693,
         "hoarr"=>8703,
         "HOARR"=>8703,
         "loarr"=>8701,
         "LOARR"=>8701,
         "roarr"=>8702,
         "ROARR"=>8702,
         "xlarr"=>10229,
         "XLARR"=>10229,
         "xrarr"=>10230,
         "XRARR"=>10230,
         "xharr"=>10231,
         "XHARR"=>10231,
         "xlArr"=>10232,
         "xrArr"=>10233,
         "xhArr"=>10234,
         "dzigrarr"=>10239,
         "DZIGRARR"=>10239,
         "xmap"=>10236,
         "XMAP"=>10236,
         "nvlArr"=>10498,
         "nvlarr"=>10498,
         "NVLARR"=>10498,
         "nvrArr"=>10499,
         "nvrarr"=>10499,
         "NVRARR"=>10499,
         "nvHarr"=>10500,
         "nvharr"=>10500,
         "NVHARR"=>10500,
         "Map"=>10501,
         "map"=>10501,
         "MAP"=>10501,
         "lbarr"=>10508,
         "LBARR"=>10508,
         "rbarr"=>10509,
         "RBARR"=>10509,
         "lBarr"=>10510,
         "rBarr"=>10511,
         "RBarr"=>10512,
         "DDotrahd"=>10513,
         "ddotrahd"=>10513,
         "DDOTRAHD"=>10513,
         "UpArrowBar"=>10514,
         "uparrowbar"=>10514,
         "UPARROWBAR"=>10514,
         "DownArrowBar"=>10515,
         "downarrowbar"=>10515,
         "DOWNARROWBAR"=>10515,
         "Rarrtl"=>10518,
         "rarrtl"=>10518,
         "RARRTL"=>10518,
         "latail"=>10521,
         "LATAIL"=>10521,
         "ratail"=>10522,
         "RATAIL"=>10522,
         "lAtail"=>10523,
         "rAtail"=>10524,
         "larrfs"=>10525,
         "LARRFS"=>10525,
         "rarrfs"=>10526,
         "RARRFS"=>10526,
         "larrbfs"=>10527,
         "LARRBFS"=>10527,
         "rarrbfs"=>10528,
         "RARRBFS"=>10528,
         "nwarhk"=>10531,
         "NWARHK"=>10531,
         "nearhk"=>10532,
         "NEARHK"=>10532,
         "searhk"=>10533,
         "SEARHK"=>10533,
         "swarhk"=>10534,
         "SWARHK"=>10534,
         "nwnear"=>10535,
         "NWNEAR"=>10535,
         "nesear"=>10536,
         "NESEAR"=>10536,
         "seswar"=>10537,
         "SESWAR"=>10537,
         "swnwar"=>10538,
         "SWNWAR"=>10538,
         "cudarrr"=>10549,
         "CUDARRR"=>10549,
         "ldca"=>10550,
         "LDCA"=>10550,
         "rdca"=>10551,
         "RDCA"=>10551,
         "cudarrl"=>10552,
         "CUDARRL"=>10552,
         "larrpl"=>10553,
         "LARRPL"=>10553,
         "curarrm"=>10556,
         "CURARRM"=>10556,
         "cularrp"=>10557,
         "CULARRP"=>10557,
         "rarrpl"=>10565,
         "RARRPL"=>10565,
         "harrcir"=>10568,
         "HARRCIR"=>10568,
         "Uarrocir"=>10569,
         "uarrocir"=>10569,
         "UARROCIR"=>10569,
         "lurdshar"=>10570,
         "LURDSHAR"=>10570,
         "ldrushar"=>10571,
         "LDRUSHAR"=>10571,
         "RightUpDownVector"=>10575,
         "rightupdownvector"=>10575,
         "RIGHTUPDOWNVECTOR"=>10575,
         "DownLeftRightVector"=>10576,
         "downleftrightvector"=>10576,
         "DOWNLEFTRIGHTVECTOR"=>10576,
         "LeftUpDownVector"=>10577,
         "leftupdownvector"=>10577,
         "LEFTUPDOWNVECTOR"=>10577,
         "LeftVectorBar"=>10578,
         "leftvectorbar"=>10578,
         "LEFTVECTORBAR"=>10578,
         "RightVectorBar"=>10579,
         "rightvectorbar"=>10579,
         "RIGHTVECTORBAR"=>10579,
         "RightUpVectorBar"=>10580,
         "rightupvectorbar"=>10580,
         "RIGHTUPVECTORBAR"=>10580,
         "RightDownVectorBar"=>10581,
         "rightdownvectorbar"=>10581,
         "RIGHTDOWNVECTORBAR"=>10581,
         "DownLeftVectorBar"=>10582,
         "downleftvectorbar"=>10582,
         "DOWNLEFTVECTORBAR"=>10582,
         "DownRightVectorBar"=>10583,
         "downrightvectorbar"=>10583,
         "DOWNRIGHTVECTORBAR"=>10583,
         "LeftUpVectorBar"=>10584,
         "leftupvectorbar"=>10584,
         "LEFTUPVECTORBAR"=>10584,
         "LeftDownVectorBar"=>10585,
         "leftdownvectorbar"=>10585,
         "LEFTDOWNVECTORBAR"=>10585,
         "LeftTeeVector"=>10586,
         "leftteevector"=>10586,
         "LEFTTEEVECTOR"=>10586,
         "RightTeeVector"=>10587,
         "rightteevector"=>10587,
         "RIGHTTEEVECTOR"=>10587,
         "RightUpTeeVector"=>10588,
         "rightupteevector"=>10588,
         "RIGHTUPTEEVECTOR"=>10588,
         "RightDownTeeVector"=>10589,
         "rightdownteevector"=>10589,
         "RIGHTDOWNTEEVECTOR"=>10589,
         "DownLeftTeeVector"=>10590,
         "downleftteevector"=>10590,
         "DOWNLEFTTEEVECTOR"=>10590,
         "DownRightTeeVector"=>10591,
         "downrightteevector"=>10591,
         "DOWNRIGHTTEEVECTOR"=>10591,
         "LeftUpTeeVector"=>10592,
         "leftupteevector"=>10592,
         "LEFTUPTEEVECTOR"=>10592,
         "LeftDownTeeVector"=>10593,
         "leftdownteevector"=>10593,
         "LEFTDOWNTEEVECTOR"=>10593,
         "lHar"=>10594,
         "lhar"=>10594,
         "LHAR"=>10594,
         "uHar"=>10595,
         "uhar"=>10595,
         "UHAR"=>10595,
         "rHar"=>10596,
         "rhar"=>10596,
         "RHAR"=>10596,
         "dHar"=>10597,
         "dhar"=>10597,
         "DHAR"=>10597,
         "luruhar"=>10598,
         "LURUHAR"=>10598,
         "ldrdhar"=>10599,
         "LDRDHAR"=>10599,
         "ruluhar"=>10600,
         "RULUHAR"=>10600,
         "rdldhar"=>10601,
         "RDLDHAR"=>10601,
         "lharul"=>10602,
         "LHARUL"=>10602,
         "llhard"=>10603,
         "LLHARD"=>10603,
         "rharul"=>10604,
         "RHARUL"=>10604,
         "lrhard"=>10605,
         "LRHARD"=>10605,
         "udhar"=>10606,
         "UDHAR"=>10606,
         "duhar"=>10607,
         "DUHAR"=>10607,
         "RoundImplies"=>10608,
         "roundimplies"=>10608,
         "ROUNDIMPLIES"=>10608,
         "erarr"=>10609,
         "ERARR"=>10609,
         "simrarr"=>10610,
         "SIMRARR"=>10610,
         "larrsim"=>10611,
         "LARRSIM"=>10611,
         "rarrsim"=>10612,
         "RARRSIM"=>10612,
         "rarrap"=>10613,
         "RARRAP"=>10613,
         "ltlarr"=>10614,
         "LTLARR"=>10614,
         "gtrarr"=>10616,
         "GTRARR"=>10616,
         "subrarr"=>10617,
         "SUBRARR"=>10617,
         "suplarr"=>10619,
         "SUPLARR"=>10619,
         "lfisht"=>10620,
         "LFISHT"=>10620,
         "rfisht"=>10621,
         "RFISHT"=>10621,
         "ufisht"=>10622,
         "UFISHT"=>10622,
         "dfisht"=>10623,
         "DFISHT"=>10623,                   
         x=>{
             // check if we are a bare unicode number
             let x = x.as_bytes();
             if x[0] == b'#'{
                 if x.len()>1 && x[1] == b'x'{ // hex
                     if let Ok(utf8) = std::str::from_utf8(&x[2..]){
                         if let Ok(num) = i64::from_str_radix(utf8, 16){
                             num as u32
                         }
                         else{
                             return Err("Cannot parse hex html entity".into())
                         }
                     }
                     else{
                         return Err("Cannot parse hex html entity".into())
                     }
                 }
                 else{
                     if let Ok(utf8) = std::str::from_utf8(&x[1..]){
                         if let Ok(num) = utf8.parse::<i64>(){
                             num as u32
                         }
                         else{
                             return Err("Cannot parse digit html entity".into())
                         }
                     }
                     else{
                         return Err("Cannot parse digit html entity".into())
                     }
                 }
             }
             else{
                 return Err("unknown html entity".into())
             }
         }
     })
 }