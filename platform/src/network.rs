use crate::makepad_micro_serde::*;
use crate::makepad_live_id::*;

use std::collections::BTreeMap;
use std::str;
#[derive(PartialEq, Debug)]
pub struct HttpRequest {
    pub id: LiveId,
    pub url: String,
    pub method: Method,
    pub headers: BTreeMap<String, Vec<String>>,
    pub body: Option<Vec<u8>>,
}

impl HttpRequest { 
    pub fn new(id: LiveId, url: String, method: Method) -> Self {
        HttpRequest {
            id,
            url,
            method,
            headers: BTreeMap::new(),
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
            headers_string.push_str(&format!("{}: {}\r\n", key, value.join(",")));
        }
        headers_string
    }

    pub fn set_raw_body(&mut self, body: Vec<u8>) {
        self.body = Some(body);
    }

    pub fn set_body<T: SerJson>(&mut self, body: T) {
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
    pub id: LiveId,
    pub status_code: u16,
    pub headers: BTreeMap<String, Vec<String>>,
    pub body: Option<Vec<u8>>,
}

impl HttpResponse {
    pub fn new(id: LiveId, status_code: u16, string_headers: String, body: Option<Vec<u8>>) -> Self {
        HttpResponse {
            id,
            status_code,
            headers: HttpResponse::parse_headers(string_headers),
            body
        }
    }

    pub fn set_header(&mut self, name: String, value: String) {
        let entry = self.headers.entry(name).or_insert(Vec::new());
        entry.push(value);
    }

    pub fn get_raw_body(&self) -> Option<&Vec<u8>> {
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
    pub fn get_json_body_as<T: DeJson>(&self) -> Option<T> { 
        if let Some(body) = self.body.as_ref() {
            let json = str::from_utf8(&body).unwrap();
            let deserialized: T = DeJson::deserialize_json(&json).unwrap();
            
            Some(deserialized)
        } else {
            None
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