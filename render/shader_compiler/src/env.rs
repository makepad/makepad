use makepad_live_parser::LiveError;
use makepad_live_parser::Span;
//use makepad_live_parser::LiveValue;
use makepad_live_parser::LiveErrorOrigin;
use makepad_live_parser::live_error_origin;
//use crate::shaderast::IdentPath;
use crate::shaderast::Ident;
use crate::shaderast::Scope;
use crate::shaderast::Sym;
use crate::shaderast::ClosureDef;
use std::collections::hash_map::Entry;
use std::cell::RefCell;


