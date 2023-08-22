use crate::makepad_micro_serde::*;
use crate::makepad_live_id::*;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::collections::BTreeMap;
use std::str;
use crate::event::Event;

#[derive(Clone, Debug)]
pub struct NetworkResponseEvent {
    pub id: LiveId,
    pub response: NetworkResponse,
}

#[derive(Clone, Debug)]
pub enum NetworkResponse{
    HttpRequestError(String),
    HttpResponse(HttpResponse),
    HttpProgress{loaded:u32, total:u32},
    WebSocketClose,
    WebSocketOpen,
    WebSocketError(String),
    WebSocketString(String),
    WebSocketBinary(Vec<u8>)
}

pub struct NetworkResponseIter<I> {
    iter: Option<I>,
}

impl<I> Iterator for NetworkResponseIter<I> where I: Iterator {
    type Item = I::Item;
    
    fn next(&mut self) -> Option<I::Item> {
        match &mut self.iter{
            None=>None,
            Some(v)=>v.next()
        }
    }
}

impl Event{
    pub fn network_responses(&self) -> NetworkResponseIter<std::slice::Iter<'_, NetworkResponseEvent>>{
        match self{
            Event::NetworkResponses(responses)=>{
                NetworkResponseIter{
                    iter:Some(responses.iter())
                }
            }
            _=>{
                // return empty thing
                NetworkResponseIter{iter:None}
            }
        } 
    }
}

pub struct NetworkResponseChannel {
    pub receiver: Receiver<NetworkResponseEvent>,
    pub sender: Sender<NetworkResponseEvent>,
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
    pub request_id: LiveId,
    pub url: String,
    pub method: HttpMethod,
    pub headers: BTreeMap<String, Vec<String>>,
    pub body: Option<Vec<u8>>,
}

impl HttpRequest { 
    pub fn new(url: String, method: HttpMethod) -> Self {
        HttpRequest {
            request_id: LiveId(0),
            url,
            method,
            headers: BTreeMap::new(),
            body: None
        }
    }
    
    pub fn set_request_id(&mut self, id: LiveId){
        self.request_id = id;
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
    pub request_id: LiveId,
    pub status_code: u16,
    pub headers: BTreeMap<String, Vec<String>>,
    pub body: Option<Vec<u8>>,
}

impl HttpResponse {
    pub fn new(request_id:LiveId, status_code: u16, string_headers: String, body: Option<Vec<u8>>) -> Self {
        HttpResponse {
            request_id,
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
            let deserialized = String::from_utf8(body.to_vec()).unwrap();
            Some(deserialized)
        } else {
            None
        }
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