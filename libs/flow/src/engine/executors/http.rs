use super::{param, string_param, Executor, Poll};
use crate::engine::{unix_ms, HttpLogEntry, NetPolicy};
use crate::{Literal, Node, PortType, Value};
use makepad_strict_json::Value as JsonValue;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct HttpReq {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub content_type: String,
    pub deadline: Instant,
}

#[derive(Clone, Debug)]
pub struct HttpResp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub trait HttpSeam: Send + Sync {
    fn request(&self, req: HttpReq) -> Result<HttpResp, String>;
}

pub struct HttpExecutor {
    seam: Arc<dyn HttpSeam>,
    policy: NetPolicy,
    log: Arc<Mutex<Vec<HttpLogEntry>>>,
    receiver: Option<Receiver<(Result<HttpResp, String>, u64)>>,
    node: Option<Node>,
    url: String,
    started_ms: u64,
}

impl HttpExecutor {
    pub fn new(
        seam: Arc<dyn HttpSeam>,
        policy: NetPolicy,
        log: Arc<Mutex<Vec<HttpLogEntry>>>,
    ) -> Self {
        Self {
            seam,
            policy,
            log,
            receiver: None,
            node: None,
            url: String::new(),
            started_ms: 0,
        }
    }
}

impl Executor for HttpExecutor {
    fn start(&mut self, node: &Node, inputs: &[(String, Value)]) -> Result<(), String> {
        let url = text_input(inputs, "url")?.to_string();
        let method = string_param(node, "method").to_ascii_uppercase();
        self.started_ms = unix_ms();
        self.url = url.clone();
        let headers = headers_input(inputs)?;
        let body_value = inputs.iter().find_map(|(port, value)| (port == "body").then_some(value));
        let body = body_value.map(|value| value.bytes.to_vec()).unwrap_or_default();
        let mut content_type = string_param(node, "content_type");
        if content_type.is_empty() {
            content_type = body_value
                .map(|value| value.content_type.clone())
                .unwrap_or_default();
        }
        let req = HttpReq {
            method: method.clone(),
            url: url.clone(),
            headers,
            body,
            content_type,
            deadline: Instant::now() + Duration::from_secs(60),
        };
        let seam = self.seam.clone();
        let policy = self.policy.clone();
        let (sender, receiver) = channel();
        let spawn = std::thread::Builder::new()
            .name(format!("flow-http-{}", node.id))
            .spawn(move || {
                let started = Instant::now();
                let response = policy.check(&req.url).and_then(|_| seam.request(req));
                let _ = sender.send((response, started.elapsed().as_millis() as u64));
            });
        if let Err(error) = spawn {
            self.log.lock().unwrap().push(HttpLogEntry {
                ms: self.started_ms,
                method,
                url,
                status: None,
            });
            return Err(error.to_string());
        }
        self.receiver = Some(receiver);
        self.node = Some(node.clone());
        Ok(())
    }

    fn poll(&mut self) -> Poll {
        let Some(receiver) = self.receiver.as_ref() else {
            return Poll::Pending;
        };
        let (response, elapsed_ms) = match receiver.try_recv() {
            Ok(response) => response,
            Err(std::sync::mpsc::TryRecvError::Empty) => return Poll::Pending,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Poll::Failed("HTTP worker stopped without a response".to_string())
            }
        };
        let node = self.node.as_ref().unwrap();
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                self.log.lock().unwrap().push(HttpLogEntry {
                    ms: self.started_ms,
                    method: string_param(node, "method").to_ascii_uppercase(),
                    url: self.url.clone(),
                    status: None,
                });
                return Poll::Failed(error);
            }
        };
        self.log.lock().unwrap().push(HttpLogEntry {
            ms: self.started_ms,
            method: string_param(node, "method").to_ascii_uppercase(),
            url: self
                .url
                .clone(),
            status: Some(response.status),
        });
        if response.body.len() > 32 * 1024 * 1024 {
            return Poll::Failed("HTTP response body exceeds 32 MiB".to_string());
        }
        let accepted = accept_status(node, response.status);
        if response.status >= 400 && !accepted {
            return Poll::Failed(format!("HTTP status {}", response.status));
        }
        let out_ty = node
            .outputs
            .iter()
            .find(|output| output.name == "value")
            .map(|output| output.ty)
            .unwrap_or(PortType::Text);
        let content_type = response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let value = match out_ty {
            PortType::Text => match String::from_utf8(response.body) {
                Ok(text) => Value::text(text),
                Err(error) => return Poll::Failed(error.to_string()),
            },
            PortType::Json => match String::from_utf8(response.body) {
                Ok(text) => {
                    if let Err(error) = makepad_strict_json::parse(text.as_bytes()) {
                        return Poll::Failed(format!("invalid JSON response: {error}"));
                    }
                    Value::json(text)
                }
                Err(error) => return Poll::Failed(error.to_string()),
            },
            PortType::List => match String::from_utf8(response.body) {
                Ok(text) => Value::list(text),
                Err(error) => return Poll::Failed(error.to_string()),
            },
            ty => Value::media(ty, content_type, response.body),
        };
        let headers = JsonValue::Obj(
            response
                .headers
                .into_iter()
                .map(|(name, value)| (name, JsonValue::Str(value)))
                .collect(),
        );
        let meta = JsonValue::Obj(vec![
            ("status".to_string(), JsonValue::Int(response.status as i64)),
            ("headers".to_string(), headers),
            ("ms".to_string(), JsonValue::Int(elapsed_ms as i64)),
        ]);
        Poll::Done(vec![
            ("value".to_string(), value),
            ("meta".to_string(), Value::json(meta.to_json())),
        ])
    }

    fn cancel(&mut self) {}
}

fn text_input<'a>(inputs: &'a [(String, Value)], port: &str) -> Result<&'a str, String> {
    inputs
        .iter()
        .find_map(|(name, value)| (name == port).then_some(value.as_text()))
        .transpose()?
        .ok_or_else(|| format!("HTTP input `{port}` is missing"))
}

fn headers_input(inputs: &[(String, Value)]) -> Result<Vec<(String, String)>, String> {
    let Some(value) = inputs.iter().find_map(|(name, value)| (name == "headers").then_some(value)) else {
        return Ok(Vec::new());
    };
    let json = makepad_strict_json::parse(&value.bytes)
        .map_err(|error| format!("invalid HTTP headers JSON: {error}"))?;
    let JsonValue::Obj(fields) = json else {
        return Err("HTTP headers must be a JSON object".to_string());
    };
    fields
        .into_iter()
        .map(|(name, value)| match value {
            JsonValue::Str(value) => Ok((name, value)),
            _ => Err("HTTP header values must be strings".to_string()),
        })
        .collect()
}

fn accept_status(node: &Node, status: u16) -> bool {
    match param(node, "accept") {
        Some(Literal::Arr(values)) => values
            .iter()
            .any(|value| matches!(value, Literal::Num(code) if *code == status as f64)),
        _ => false,
    }
}
