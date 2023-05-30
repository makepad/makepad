use crate::makepad_micro_serde::*;
use crate::makepad_live_id::*;

use std::collections::HashMap;
use std::str;
#[derive(PartialEq, Debug)]
pub struct HttpRequest {
    pub id: LiveId,
    pub url: String,
    pub method: Method,
    pub headers: HashMap<String, Vec<String>>,
    pub body: Option<Vec<u8>>,
}

impl HttpRequest { 
    // TODO: a good default
    pub fn new(id: LiveId, url: String, method: Method) -> Self {
        HttpRequest {
            id,
            url,
            method,
            headers: HashMap::new(),
            body: None
        }
    }

    pub fn set_header(&mut self, name: String, value: String) {
        let entry = self.headers.entry(name).or_insert(Vec::new());
        entry.push(value);
    }

    pub fn get_headers_string(&self) -> String {
        let mut headers_string = String::new();
        for (key, value) in self.headers.iter() {
            headers_string.push_str(&format!("{}: {}\n", key, value.join(",")));
        }
        headers_string
    }

    // WIP - takes whatever the user sends like a struct and we serialize to a byte array.
    // if it's possible I'd always send the body as a byte array to java to avoid 
    // sending a generic body and doing parsing/serializing on that side.
    // if we can't rely to always send byte array in the body,
    // we could use the header's content-type and use that to know what to serialize into.
    pub fn set_body<T: DeBin + SerBin + SerJson + DeJson>(&mut self, body: T) {
       let json_body = body.serialize_json();
       let serialized_body = json_body.into_bytes();
       self.body = Some(serialized_body); 
    }
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub id: LiveId,
    pub status_code: u16,
    pub headers: HashMap<String, Vec<String>>,
    pub body: Option<Vec<u8>>,
}

impl HttpResponse {
    pub fn new(id: LiveId, status_code: u16, string_headers: String, body: Option<Vec<u8>>) -> Self {
        HttpResponse {
            id,
            status_code,
            headers: HttpResponse::parse_headers(string_headers),
            body: body
        }
    }

    pub fn get_raw_body(&self) -> Option<&Vec<u8>> {
        self.body.as_ref()
    }

    pub fn get_body<T: DeBin + SerJson + DeJson>(&self) -> Option<T> { 
        if let Some(body) = self.body.as_ref() {
            let json = str::from_utf8(&body).unwrap();
            let deserialized: T = DeJson::deserialize_json(&json).unwrap();
            
            Some(deserialized)
        } else {
            None
        }
    }

    fn parse_headers(headers_string: String) -> HashMap<String, Vec<String>> {
        let mut headers = HashMap::new();
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
pub enum Method{
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

impl Method {
    pub fn to_string(&self) -> &str {
        match self {
            Method::GET => "GET",
            Method::HEAD => "HEAD",
            Method::POST => "POST",
            Method::PUT => "PUT",
            Method::DELETE => "DELETE",
            Method::CONNECT => "CONNECT",
            Method::OPTIONS => "OPTIONS",
            Method::TRACE => "TRACE",
            Method::PATCH => "PATCH",
        }
    }
}