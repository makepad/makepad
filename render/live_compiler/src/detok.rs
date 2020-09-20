use crate::token::{Token};
use crate::error::LiveError;
use crate::ident::{Ident, IdentPath};
use crate::span::{Span, LiveBodyId};
use crate::lit::{Lit};
use crate::colors::Color;
use crate::math::*;
use crate::livestyles::LiveStyles;
use crate::livetypes::{Font, LiveId, Play, Anim, Ease, Track};

pub trait DeTokParser {
    fn ident_path_to_live_id(&self, ident_path: &IdentPath) -> LiveId;
    fn end(&self) -> usize;
    fn token_end(&self) -> usize;
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
                        let live_id = p.ident_path_to_live_id(&ident_path);
                        if let Some(float) = p.get_live_styles().base.floats.get(&live_id) {
                            return Ok(*float as $ ty);
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
        p.expect_token(Token::Ident(Ident::new("vec2"))) ?;
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
        p.expect_token(Token::Ident(Ident::new("vec3"))) ?;
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
        p.expect_token(Token::Ident(Ident::new("vec4"))) ?;
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
    }
}

impl DeTok for Color {
    fn de_tok(p: &mut dyn DeTokParser) -> Result<Color, LiveError> {
        match p.peek_token() {
            Token::Lit(Lit::Color(c)) => {
                p.skip_token();
                return Ok(c);
            },
            Token::Ident(_) => { // try to parse ident path, and read from styles
                let ident_path = p.parse_ident_path() ?;
                let live_id = p.ident_path_to_live_id(&ident_path);
                if let Some(color) = p.get_live_styles().base.colors.get(&live_id) {
                    return Ok(*color);
                }
                return Err(p.error(format!("Color {} not found", ident_path)));
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
        Track::Color {keys, ..} => {
            keys.push((time, Color::de_tok(p) ?));
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
                else if ident == Ident::new("live_id") {
                    p.skip_token();
                    p.expect_token(Token::Colon) ?;
                    let ident_path = p.parse_ident_path() ?;
                    track.set_live_id(p.ident_path_to_live_id(&ident_path));
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
                            Token::Comma=>{
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
            Token::Comma=>{
                p.skip_token();
            },
            token => {
                return Err(span.error(p, format!("Unexpected token in track {}", token)));
            }
        }
        p.accept_token(Token::Comma);
    }
}

impl DeTok for Anim {
    fn de_tok(p: &mut dyn DeTokParser) -> Result<Anim, LiveError> {
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
                            live_id: LiveId(tracks.len() as u64),
                            ease: Ease::Lin,
                            cut_init: None,
                            keys: Vec::new()
                        };
                        parse_track(p, &mut track) ?;
                        tracks.push(track);
                    }
                    else if ident == Ident::new("Vec2") {
                        let mut track = Track::Vec2 {
                            live_id: LiveId(tracks.len() as u64),
                            ease: Ease::Lin,
                            cut_init: None,
                            keys: Vec::new()
                        };
                        parse_track(p, &mut track) ?;
                        tracks.push(track);
                    }
                    else if ident == Ident::new("Vec3") {
                        let mut track = Track::Vec3 {
                            live_id: LiveId(tracks.len() as u64),
                            ease: Ease::Lin,
                            cut_init: None,
                            keys: Vec::new()
                        };
                        parse_track(p, &mut track) ?;
                        tracks.push(track);
                    }
                    else if ident == Ident::new("Vec4") {
                        let mut track = Track::Vec4 {
                            live_id: LiveId(tracks.len() as u64),
                            ease: Ease::Lin,
                            cut_init: None,
                            keys: Vec::new()
                        };
                        parse_track(p, &mut track) ?;
                        tracks.push(track);
                    }
                    else if ident == Ident::new("Color") {
                        let mut track = Track::Color {
                            live_id: LiveId(tracks.len() as u64),
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