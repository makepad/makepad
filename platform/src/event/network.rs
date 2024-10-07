use crate::makepad_micro_serde::*;
use crate::makepad_live_id::*;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::collections::BTreeMap;
use std::str;

#[derive(Clone, Debug)]
pub struct NetworkResponseItem{
    pub request_id: LiveId,
    pub response: NetworkResponse,
}

pub type NetworkResponsesEvent = Vec<NetworkResponseItem>;

#[derive(Clone, Debug)]
pub struct HttpError{
    pub message: String,
    pub metadata_id: LiveId
}


#[derive(Clone, Debug)]
pub struct HttpProgress{
    pub loaded:u64, 
    pub total:u64,
}

#[derive(Clone, Debug)]
pub enum NetworkResponse{
    HttpRequestError(HttpError),
    HttpResponse(HttpResponse),
    HttpStreamResponse(HttpResponse),
    HttpStreamComplete(HttpResponse),
    HttpProgress(HttpProgress),
}
/*
pub struct NetworkResponseIter<I> {
    iter: Option<I>,
}
*/
/*
impl<I> Iterator for NetworkResponseIter<I> where I: Iterator {
    type Item = I::Item;
    
    fn next(&mut self) -> Option<I::Item> {
        match &mut self.iter{
            None=>None,
            Some(v)=>v.next()
        }
    }
}*/

pub struct NetworkResponseChannel {
    pub receiver: Receiver<NetworkResponseItem>,
    pub sender: Sender<NetworkResponseItem>,
}

impl Default for NetworkResponseChannel {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver
        }
    }
}


#[derive(PartialEq, Debug)]
pub struct HttpRequest {
    pub metadata_id: LiveId,
    pub url: String,
    pub method: HttpMethod,
    pub headers: BTreeMap<String, Vec<String>>,
    pub ignore_ssl_cert: bool,
    pub is_streaming: bool,
    pub body: Option<Vec<u8>>,
}

#[derive(Debug)]
pub struct SplitUrl<'a>{
    pub proto: &'a str,
    pub host: &'a str,
    pub port: &'a str,
    pub file: &'a str,
    pub hash: &'a str,
}

impl HttpRequest { 
    pub fn new(url: String, method: HttpMethod) -> Self {
        HttpRequest {
            metadata_id: LiveId(0),
            url,
            method,
            is_streaming: false,
            ignore_ssl_cert: false,
            headers: BTreeMap::new(),
            body: None
        }
    }

    pub fn split_url(&self)->SplitUrl{
        let (proto, rest) = self.url.split_once("://").unwrap_or((&self.url, "http://"));
        let (host, port, rest) = if let Some((host, rest)) = rest.split_once(":"){
            let (port, rest) = rest.split_once("/").unwrap_or((rest, ""));
            (host, port, rest)
        }
        else{
            let (host, rest) = rest.split_once("/").unwrap_or((rest, ""));
            (host,match proto{
                "http"|"ws"=>"80",
                "https"|"wss"=>"443",
                _=>"80"
            },rest)
        };
        let (file, hash) = rest.split_once("#").unwrap_or((rest, ""));
        return SplitUrl{
            proto,
            host,
            port,
            file, 
            hash
        }
    }
    
    pub fn set_ignore_ssl_cert(&mut self){
        self.ignore_ssl_cert = true
    }
    
    pub fn set_is_streaming(&mut self){
        self.is_streaming = true
    }
    
    pub fn set_metadata_id(&mut self, id: LiveId){
        self.metadata_id = id;
    }
    
    pub fn set_header(&mut self, name: String, value: String) {
        let entry = self.headers.entry(name).or_insert(Vec::new());
        entry.push(value);
    }

    pub fn get_headers_string(&self) -> String {
        let mut headers_string = String::new();
        for (key, value) in self.headers.iter() {
            headers_string.push_str(&format!("{}: {}\r\n", key, value.join(",")));
        }
        headers_string
    }

    pub fn set_body(&mut self, body: Vec<u8>) {
        self.body = Some(body);
    }

    pub fn set_body_string(&mut self, v: &str) {
        self.body = Some(v.as_bytes().to_vec());
    }

    pub fn set_json_body<T: SerJson>(&mut self, body: T) {
       let json_body = body.serialize_json();
       let serialized_body = json_body.into_bytes();
       self.body = Some(serialized_body); 
    }

    pub fn set_string_body(&mut self, body: String) {
        self.body = Some(body.into_bytes());
    }
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub metadata_id: LiveId,
    pub status_code: u16,
    pub headers: BTreeMap<String, Vec<String>>,
    pub body: Option<Vec<u8>>,
}

impl HttpResponse {
    pub fn new(metadata_id: LiveId, status_code: u16, string_headers: String, body: Option<Vec<u8>>) -> Self {
        HttpResponse {
            metadata_id,
            status_code,
            headers: HttpResponse::parse_headers(string_headers),
            body
        }
    }

    pub fn set_header(&mut self, name: String, value: String) {
        let entry = self.headers.entry(name).or_insert(Vec::new());
        entry.push(value);
    }

    pub fn get_body(&self) -> Option<&Vec<u8>> {
        self.body.as_ref()
    }

    pub fn get_string_body(&self) -> Option<String> {
        if let Some(body) = self.body.as_ref() {
            if let Ok(utf8) = String::from_utf8(body.to_vec()){
                return Some(utf8)
            }
        }
        None
    }

    // Todo: a more generic function that supports serialization into rust structs from other MIME types
    pub fn get_json_body<T: DeJson>(&self) -> Result<T, DeJsonErr> { 
        if let Some(body) = self.body.as_ref() {
            let json = str::from_utf8(&body).unwrap();
            DeJson::deserialize_json(&json)
        } else {
            Err(DeJsonErr{
                msg:"No body present".to_string(),
                line:0,
                col:0
            })
        }
    }

    fn parse_headers(headers_string: String) -> BTreeMap<String, Vec<String>> {
        let mut headers = BTreeMap::new();
        for line in headers_string.lines() {
            let mut split = line.split(":");
            let key = split.next().unwrap();
            let values = split.next().unwrap().to_string();
            for val in values.split(",") {
                let entry = headers.entry(key.to_string()).or_insert(Vec::new());
                entry.push(val.to_string());
            }
        }
        headers
    }
}

#[derive(PartialEq, Debug)]
pub enum HttpMethod{
    GET,
    HEAD,
    POST,
    PUT,
    DELETE,
    CONNECT,
    OPTIONS,
    TRACE,
    PATCH
}

impl HttpMethod {
    pub fn to_string(&self) -> &str {
        match self {
            Self::GET => "GET",
            Self::HEAD => "HEAD",
            Self::POST => "POST",
            Self::PUT => "PUT",
            Self::DELETE => "DELETE",
            Self::CONNECT => "CONNECT",
            Self::OPTIONS => "OPTIONS",
            Self::TRACE => "TRACE",
            Self::PATCH => "PATCH",
        }
    }
}