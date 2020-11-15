use crate::token::{Token, TokenWithSpan};
use crate::error::LiveError;
use crate::ident::{Ident, IdentPath, QualifiedIdentPath};
use crate::span::{Span, LiveBodyId};
use crate::lit::Lit;
use crate::ty::TyLit;
use crate::math::*;
use crate::livestyles::{LiveStyles, LiveStyle};
use crate::livetypes::{Font, LiveItemId, Play, Anim, Ease, Track};
use std::iter::Cloned;
use std::slice::Iter;


pub trait DeTokParser {
    fn qualify_ident_path(&self, ident_path: &IdentPath) -> QualifiedIdentPath;
    fn end(&self) -> usize;
    fn token_end(&self) -> usize;
    fn peek_span(&self) -> Span;
    fn peek_token(&self) -> Token;
    fn skip_token(&mut self);
    fn error(&mut self, msg: String) -> LiveError;
    fn parse_ident(&mut self) -> Result<Ident, LiveError>;
    fn parse_ident_path(&mut self) -> Result<IdentPath, LiveError>;
    fn accept_token(&mut self, token: Token) -> bool;
    fn expect_token(&mut self, expected: Token) -> Result<(), LiveError>;
    fn accept_ident(&mut self, ident_str: &str) -> bool;
    fn expect_ident(&mut self, ident_str: &str) -> Result<(), LiveError>;
    fn get_live_styles(&mut self) -> &mut LiveStyles;
    fn error_not_splattable(&mut self, what: &str) -> LiveError;
    fn error_missing_prop(&mut self, what: &str) -> LiveError;
    fn error_enum(&mut self, ident: Ident, what: &str) -> LiveError;
    fn begin_span(&self) -> SpanTracker;
    fn clear_token_clone(&mut self);
    fn get_token_clone(&mut self) -> Vec<TokenWithSpan>;
}

pub struct DeTokParserImpl<'a> {
    pub live_styles: &'a mut LiveStyles,
    pub token_clone: Vec<TokenWithSpan>,
    pub tokens_with_span: Cloned<Iter<'a, TokenWithSpan >>,
    pub token_with_span: TokenWithSpan,
    pub end: usize,
}

impl<'a> DeTokParserImpl<'a>{
    pub fn new(tokens_with_span:&'a [TokenWithSpan], live_styles:&'a mut LiveStyles)->Self{
        let mut tokens_with_span = tokens_with_span.iter().cloned();
        let token_with_span = tokens_with_span.next().unwrap();
        DeTokParserImpl {
            live_styles: live_styles,
            token_clone: Vec::new(),
            tokens_with_span,
            token_with_span,
            end: 0,
        }
    }
}

impl<'a> DeTokParser for DeTokParserImpl<'a> {
    
    fn clear_token_clone(&mut self) {
        self.token_clone.truncate(0);
    }
    
    fn get_token_clone(&mut self) -> Vec<TokenWithSpan> {
        let mut new_token_storage = Vec::new();
        std::mem::swap(&mut new_token_storage, &mut self.token_clone);
        new_token_storage.push(TokenWithSpan{
            token:Token::Eof,
            span:self.token_with_span.span
        });
        return new_token_storage;
    }
     
    fn peek_span(&self) -> Span{
        self.token_with_span.span
    } 
     
    fn peek_token(&self) -> Token {
        self.token_with_span.token
    }
    
    fn skip_token(&mut self) {
        self.end = self.token_with_span.span.end;
        self.token_clone.push(self.token_with_span);
        self.token_with_span = self.tokens_with_span.next().unwrap();
    }
    
    fn error(&mut self, message: String) -> LiveError {
        LiveError {
            span: Span {
                live_body_id: self.token_with_span.span.live_body_id,
                start: self.token_with_span.span.start,
                end: self.token_with_span.span.end,
            },
            message,
        }
    }
    
    fn error_missing_prop(&mut self, what: &str) -> LiveError {
        self.error(format!("Error missing property {}", what))
    }
    
    fn error_not_splattable(&mut self, what: &str) -> LiveError {
        self.error(format!("Error type {} not splattable", what))
    }
    
    fn error_enum(&mut self, ident: Ident, what: &str) -> LiveError {
        self.error(format!("Error missing {} for enum {}", ident.to_string(), what))
    }
    
    fn parse_ident(&mut self) -> Result<Ident, LiveError> {
        match self.peek_token() {
            Token::Ident(ident) => {
                self.skip_token();
                Ok(ident)
            }
            token => Err(self.error(format!("expected ident, unexpected token `{}`", token))),
        }
    }
    
    fn parse_ident_path(&mut self) -> Result<IdentPath, LiveError> {
        let mut ident_path = IdentPath::default();
        let span = self.begin_span();
        match self.peek_token() {
            
            Token::Ident(ident) => {
                self.skip_token();
                ident_path.push(ident);
            },
            token => {
                return Err(span.error(self, format!("expected ident_path, unexpected token `{}`", token).into()));
            }
        };
        
        loop {
            if !self.accept_token(Token::PathSep) {
                return Ok(ident_path);
            }
            match self.peek_token() {
                Token::Ident(ident) => {
                    self.skip_token();
                    if !ident_path.push(ident) {
                        return Err(span.error(self, format!("identifier too long `{}`", ident_path).into()));
                    }
                },
                _ => {
                    return Ok(ident_path);
                }
            }
        }
    }
    
    fn end(&self) -> usize {
        self.end
    }
    
    fn token_end(&self) -> usize {
        self.token_with_span.span.end
    }
    
    fn accept_token(&mut self, token: Token) -> bool {
        if self.peek_token() != token {
            return false;
        }
        self.skip_token();
        true
    }
    
    fn expect_token(&mut self, expected: Token) -> Result<(), LiveError> {
        let actual = self.peek_token();
        if actual != expected {
            return Err(self.error(format!("expected {} unexpected token `{}`", expected, actual)));
        }
        self.skip_token();
        Ok(())
    }
    
    fn accept_ident(&mut self, ident_str: &str) -> bool {
        if let Token::Ident(ident) = self.peek_token() {
            if ident == Ident::new(ident_str) {
                self.skip_token();
                return true
            }
        }
        false
    }
    
    fn expect_ident(&mut self, ident_str: &str) -> Result<(), LiveError> {
        let actual = self.peek_token();
        if let Token::Ident(ident) = actual {
            if ident == Ident::new(ident_str) {
                self.skip_token();
                return Ok(())
            }
        }
        return Err(self.error(format!("expected {} unexpected token `{}`", ident_str, actual)));
    }
    
    fn begin_span(&self) -> SpanTracker {
        SpanTracker {
            live_body_id: self.token_with_span.span.live_body_id,
            start: self.token_with_span.span.start,
        }
    }
    fn qualify_ident_path(&self, ident_path: &IdentPath) -> QualifiedIdentPath {
        let module_path = &self.live_styles.live_bodies[self.token_with_span.span.live_body_id.0].module_path;
        ident_path.qualify(&module_path)
    }
    
    fn get_live_styles(&mut self) -> &mut LiveStyles {
        self.live_styles
    }
}



pub struct SpanTracker {
    pub live_body_id: LiveBodyId,
    pub start: usize,
}

impl SpanTracker {
    pub fn end<F, R>(&self, parser: &dyn DeTokParser, f: F) -> R
    where
    F: FnOnce(Span) -> R,
    {
        f(Span {
            live_body_id: self.live_body_id,
            start: self.start,
            end: parser.end(),
        })
    }
    
    pub fn error(&self, parser: &dyn DeTokParser, message: String) -> LiveError {
        LiveError {
            span: Span {
                live_body_id: self.live_body_id,
                start: self.start,
                end: parser.token_end(),
            },
            message,
        }
    }
}


pub trait DeTok: Sized {
    fn de_tok(p: &mut dyn DeTokParser) -> Result<Self,
    LiveError>;
}

pub trait DeTokSplat: Sized {
    fn de_tok_splat(p: &mut dyn DeTokParser) -> Result<Self,
    LiveError>;
}


// we now have to implement DeTok for all our integer types
// float types, bools, etc

macro_rules!impl_de_tok_for_float {
    ( $ ty: ident) => {
        impl DeTok for $ ty {
            fn de_tok(p: &mut dyn DeTokParser) -> Result< $ ty,
            LiveError> {
                match p.peek_token() {
                    Token::Lit(lit) => {
                        match lit {
                            Lit::Int(i) => {
                                p.skip_token();
                                return Ok(i as $ ty)
                            },
                            Lit::Float(i) => {
                                p.skip_token();
                                return Ok(i as $ ty)
                            },
                            _ => ()
                        }
                    },
                    Token::Ident(_) => { // try to parse ident path, and read from styles
                        let ident_path = p.parse_ident_path() ?;
                        let qualified_ident_path = p.qualify_ident_path(&ident_path);
                        let live_item_id = qualified_ident_path.to_live_item_id();
                        //p.register_dependency(live_id);
                        if let Some(float) = p.get_live_styles().floats.get(&live_item_id) {
                            return Ok(float.value as $ ty);
                        }
                        return Err(p.error(format!("Float {} not found", ident_path)));
                    },
                    _ => ()
                }
                Err(p.error(format!("Expected float literal")))
            }
        }
    };
}

macro_rules!impl_de_tok_for_int {
    ( $ ty: ident) => {
        impl DeTok for $ ty {
            fn de_tok(p: &mut dyn DeTokParser) -> Result< $ ty,
            LiveError> {
                match p.peek_token() {
                    Token::Lit(lit) => {
                        match lit {
                            Lit::Int(i) => {
                                p.skip_token();
                                return Ok(i as $ ty)
                            },
                            _ => ()
                        }
                    },
                    _ => ()
                }
                Err(p.error(format!("Expected integer literal")))
            }
        }
    };
}


impl_de_tok_for_float!(f64);
impl_de_tok_for_float!(f32);

impl_de_tok_for_int!(isize);
impl_de_tok_for_int!(usize);
impl_de_tok_for_int!(u64);
impl_de_tok_for_int!(i64);
impl_de_tok_for_int!(u32);
impl_de_tok_for_int!(i32);
impl_de_tok_for_int!(u16);
impl_de_tok_for_int!(i16);

impl DeTok for bool {
    fn de_tok(p: &mut dyn DeTokParser) -> Result<bool, LiveError> {
        match p.peek_token() {
            Token::Lit(lit) => {
                match lit {
                    Lit::Int(i) => {
                        p.skip_token();
                        return Ok(if i > 0 {true} else {false})
                    },
                    Lit::Bool(b) => {
                        p.skip_token();
                        return Ok(b)
                    },
                    _ => ()
                }
            },
            _ => ()
        }
        Err(p.error(format!("Expected integer literal")))
    }
}

impl<T> DeTok for Option<T> where T: DeTok {
    fn de_tok(p: &mut dyn DeTokParser) -> Result<Option<T>, LiveError> {
        // if ident is None then its none,m
        if let Token::Ident(ident) = p.peek_token() {
            if ident == Ident::new("None") {
                p.skip_token();
                return Ok(None)
            }
        }
        Ok(Some(DeTok::de_tok(p) ?))
        //otherwise detok the
    }
}

impl DeTok for Vec2 {
    fn de_tok(p: &mut dyn DeTokParser) -> Result<Vec2, LiveError> {
       p.expect_token(Token::TyLit(TyLit::Vec2)) ?;
        p.expect_token(Token::LeftParen) ?;
        let x = f32::de_tok(p) ?;
        p.expect_token(Token::Comma) ?;
        let y = f32::de_tok(p) ?;
        p.accept_token(Token::Comma);
        p.expect_token(Token::RightParen) ?;
        Ok(Vec2 {x, y})
    }
}

impl DeTok for Vec3 {
    fn de_tok(p: &mut dyn DeTokParser) -> Result<Vec3, LiveError> {
        p.expect_token(Token::TyLit(TyLit::Vec3)) ?;
        p.expect_token(Token::LeftParen) ?;
        let x = f32::de_tok(p) ?;
        p.expect_token(Token::Comma) ?;
        let y = f32::de_tok(p) ?;
        p.expect_token(Token::Comma) ?;
        let z = f32::de_tok(p) ?;
        p.accept_token(Token::Comma);
        p.expect_token(Token::RightParen) ?;
        Ok(Vec3 {x, y, z})
    }
}

impl DeTok for Vec4 {
    fn de_tok(p: &mut dyn DeTokParser) -> Result<Vec4, LiveError> {
        match p.peek_token() {
            Token::TyLit(TyLit::Vec4)=>{
                 p.expect_token(Token::TyLit(TyLit::Vec4)) ?;
                p.expect_token(Token::LeftParen) ?;
                let x = f32::de_tok(p) ?;
                p.expect_token(Token::Comma) ?;
                let y = f32::de_tok(p) ?;
                p.expect_token(Token::Comma) ?;
                let z = f32::de_tok(p) ?;
                p.expect_token(Token::Comma) ?;
                let w = f32::de_tok(p) ?;
                p.accept_token(Token::Comma);
                p.expect_token(Token::RightParen) ?;
                Ok(Vec4 {x, y, z, w})
            },
            Token::Lit(Lit::Vec4(c)) => {
                p.skip_token();
                return Ok(c);
            },
            Token::Ident(_) => { // try to parse ident path, and read from styles
                let ident_path = p.parse_ident_path() ?;
                let qualified_ident_path = p.qualify_ident_path(&ident_path);
                let live_item_id = qualified_ident_path.to_live_item_id();
                if let Some(color) = p.get_live_styles().vec4s.get(&live_item_id) {
                    return Ok(*color);
                }
                return Err(p.error(format!("Vec4 {} not found", ident_path)));
            },
            token => {
                return Err(p.error(format!("Expected color {}", token)));
            }
        }
    }
}

impl DeTok for Font {
    fn de_tok(p: &mut dyn DeTokParser) -> Result<Font, LiveError> {
        // simply parse a string
        match p.peek_token() {
            Token::String(ident) => {
                p.skip_token();
                return Ok(p.get_live_styles().get_or_insert_font_by_ident(ident))
            }
            _ => ()
        }
        Err(p.error(format!("Expected integer literal")))
    }
}


fn parse_track_rhs(p: &mut dyn DeTokParser, time: f64, track: &mut Track) -> Result<(), LiveError> {
    match track {
        Track::Float {keys, ..} => {
            keys.push((time, f32::de_tok(p) ?));
        },
        Track::Vec2 {keys, ..} => {
            keys.push((time, Vec2::de_tok(p) ?));
        }
        Track::Vec3 {keys, ..} => {
            keys.push((time, Vec3::de_tok(p) ?));
        }
        Track::Vec4 {keys, ..} => {
            keys.push((time, Vec4::de_tok(p) ?));
        }
    }
    Ok(())
}

fn parse_track(p: &mut dyn DeTokParser, track: &mut Track) -> Result<(), LiveError> {
    p.expect_token(Token::LeftBrace) ?;
    loop {
        let span = p.begin_span();
        match p.peek_token() {
            Token::Ident(ident) => { // ease
                if ident == Ident::new("ease") {
                    p.skip_token();
                    p.expect_token(Token::Colon) ?;
                    track.set_ease(Ease::de_tok(p) ?);
                }
                else if ident == Ident::new("bind_to") {
                    p.skip_token();
                    p.expect_token(Token::Colon) ?;
                    let ident_path = p.parse_ident_path() ?;
                    let qualified_ident_path = p.qualify_ident_path(&ident_path);
                    let live_item_id = qualified_ident_path.to_live_item_id();
                    track.set_bind_to(live_item_id);
                }
                else if ident == Ident::new("keys") {
                    p.skip_token();
                    p.expect_token(Token::Colon) ?;
                    p.expect_token(Token::LeftBrace) ?;
                    loop {
                        let span = p.begin_span();
                        match p.peek_token() {
                            Token::Lit(Lit::Int(i)) => { // integer time
                                p.skip_token();
                                p.expect_token(Token::Colon) ?;
                                // now lets parse the RHS
                                parse_track_rhs(p, i as f64, track) ?;
                            },
                            Token::Lit(Lit::Float(f)) => { // float time
                                p.skip_token();
                                p.expect_token(Token::Colon) ?;
                                parse_track_rhs(p, f as f64, track) ?;
                            },
                            Token::RightBrace => {
                                p.skip_token();
                                break;
                            },
                            Token::Comma => {
                                p.skip_token();
                            },
                            token => {
                                return Err(span.error(p, format!("Unexpected token in track keys {}", token)));
                            }
                        }
                    }
                }
                else {
                    return Err(span.error(p, format!("Invalid key for track {}", ident)));
                }
            },
            Token::RightBrace => {
                p.skip_token();
                return Ok(());
            },
            Token::Comma => {
                p.skip_token();
            },
            token => {
                return Err(span.error(p, format!("Unexpected token in track {}", token)));
            }
        }
        p.accept_token(Token::Comma);
    }
}


impl DeTok for LiveStyle {
    fn de_tok(p: &mut dyn DeTokParser) -> Result<LiveStyle, LiveError> {
        let mut live_style = LiveStyle::default();
        if let Token::Ident(ident) = p.peek_token(){
            if ident == Ident::new("Style"){
                p.skip_token();
            }
        }
        p.expect_token(Token::LeftBrace) ?;
        loop {
            if p.accept_token(Token::RightBrace){
                return Ok(live_style);
            }
            let from = p.parse_ident_path() ?;
            let from_live_item_id = p.qualify_ident_path(&from).to_live_item_id();
            p.expect_token(Token::Colon)?;
            let to = p.parse_ident_path() ? ;
            let to_live_item_id = p.qualify_ident_path(&to).to_live_item_id();
            p.expect_token(Token::Semi)?;
            live_style.remap.insert(from_live_item_id, to_live_item_id);
        }
    }
}

impl DeTok for Anim {
    fn de_tok(p: &mut dyn DeTokParser) -> Result<Anim, LiveError> {
        if let Token::Ident(ident) = p.peek_token(){
            if ident == Ident::new("Anim"){
                p.skip_token();
            }
        }
        p.expect_token(Token::LeftBrace) ?;
        let mut play = Play::Cut {duration: 1.0};
        let mut tracks = Vec::new();
        // parse all the crap in it.
        loop {
            
            if p.accept_token(Token::RightBrace) {
                return Ok(Anim {
                    play: play,
                    tracks: tracks
                })
            }
            
            let ident = p.parse_ident() ?;
            
            if ident == Ident::new("play") {
                p.expect_token(Token::Colon) ?;
                play = Play::de_tok(p) ?;
            }
            else if ident == Ident::new("tracks") {
                p.expect_token(Token::Colon) ?;
                p.expect_token(Token::LeftBracket) ?;
                loop {
                    if p.accept_token(Token::RightBracket) {
                        break;
                    }
                    if p.accept_token(Token::Ident(Ident::new("Track"))) {
                        p.expect_token(Token::PathSep) ?;
                    }
                    
                    let span = p.begin_span();
                    let ident = p.parse_ident() ?;
                    
                    if ident == Ident::new("Float") {
                        let mut track = Track::Float {
                            bind_to: LiveItemId(tracks.len() as u64),
                            ease: Ease::Lin,
                            cut_init: None,
                            keys: Vec::new()
                        };
                        parse_track(p, &mut track) ?;
                        tracks.push(track);
                    }
                    else if ident == Ident::new("Vec2") {
                        let mut track = Track::Vec2 {
                            bind_to: LiveItemId(tracks.len() as u64),
                            ease: Ease::Lin,
                            cut_init: None,
                            keys: Vec::new()
                        };
                        parse_track(p, &mut track) ?;
                        tracks.push(track);
                    }
                    else if ident == Ident::new("Vec3") {
                        let mut track = Track::Vec3 {
                            bind_to: LiveItemId(tracks.len() as u64),
                            ease: Ease::Lin,
                            cut_init: None,
                            keys: Vec::new()
                        };
                        parse_track(p, &mut track) ?;
                        tracks.push(track);
                    }
                    else if ident == Ident::new("Vec4") {
                        let mut track = Track::Vec4 {
                            bind_to: LiveItemId(tracks.len() as u64),
                            ease: Ease::Lin,
                            cut_init: None,
                            keys: Vec::new()
                        };
                        parse_track(p, &mut track) ?;
                        tracks.push(track);
                    }
                    else {
                        return Err(span.error(p, format!("Unexpected track type {}", ident)));
                    }
                    
                    p.accept_token(Token::Comma);
                }
            }
            
            p.accept_token(Token::Comma);
            
        }
    }
}