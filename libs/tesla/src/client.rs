use crate::data::*;
use makepad_micro_serde::*;
use makepad_widgets::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const AUTH_TOKEN_URL: &str = "https://fleet-auth.prd.vn.cloud.tesla.com/oauth2/v3/token";

/// Refresh the access token this many seconds before it actually expires.
const TOKEN_EXPIRY_MARGIN: u64 = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TeslaRegion {
    Na,
    #[default]
    Eu,
    Cn,
}

impl TeslaRegion {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "na" => Some(Self::Na),
            "eu" => Some(Self::Eu),
            "cn" => Some(Self::Cn),
            _ => None,
        }
    }

    pub fn base_url(self) -> &'static str {
        match self {
            Self::Na => "https://fleet-api.prd.na.vn.cloud.tesla.com",
            Self::Eu => "https://fleet-api.prd.eu.vn.cloud.tesla.com",
            Self::Cn => "https://fleet-api.prd.cn.vn.cloud.tesla.cn",
        }
    }
}

/// The credentials file. See libs/tesla/README.md for how to obtain the values.
/// The client rewrites this file whenever Tesla rotates the refresh token, and
/// caches the short-lived access token in it across restarts.
#[derive(SerJson, DeJson, Debug, Clone)]
pub struct TeslaCredentials {
    pub client_id: String,
    /// Only needed if Tesla rejects refresh without it (confidential clients).
    pub client_secret: Option<String>,
    pub refresh_token: String,
    /// "na" | "eu" | "cn"
    pub region: String,
    pub access_token: Option<String>,
    /// Unix seconds after which access_token is stale.
    pub access_token_expires: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum TeslaError {
    Credentials(String),
    Auth(String),
    Http { status: u16, message: String },
    Network(String),
    Parse(String),
}

impl std::fmt::Display for TeslaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Credentials(m) => write!(f, "tesla credentials: {}", m),
            Self::Auth(m) => write!(f, "tesla auth: {}", m),
            Self::Http { status, message } => write!(f, "tesla http {}: {}", status, message),
            Self::Network(m) => write!(f, "tesla network: {}", m),
            Self::Parse(m) => write!(f, "tesla parse: {}", m),
        }
    }
}

/// What the client hands back from handle_event.
#[derive(Debug, Clone)]
pub enum TeslaAction {
    /// Result of request_vehicles.
    Vehicles(Vec<Vehicle>),
    /// Result of request_charge_state / request_vehicle_data.
    VehicleData { vin: String, data: VehicleData },
    /// Result of request_wake_up; state is e.g. "waking"/"online".
    WakeUp { vin: String, state: Option<String> },
    /// The car is asleep/offline; issue request_wake_up (costs credits) or retry later.
    VehicleAsleep { vin: String },
    /// A fresh access token was obtained (and the credentials file rewritten).
    TokenRefreshed,
    Error(TeslaError),
}

#[derive(Clone, Debug)]
enum ApiCall {
    Vehicles,
    VehicleData { vin: String, endpoints: String },
    WakeUp { vin: String },
}

impl ApiCall {
    fn vin(&self) -> Option<&str> {
        match self {
            Self::Vehicles => None,
            Self::VehicleData { vin, .. } | Self::WakeUp { vin } => Some(vin),
        }
    }
}

#[derive(Clone, Debug)]
struct PendingCall {
    call: ApiCall,
    auth_retried: bool,
}

/// Event-driven Tesla Fleet API client.
///
/// Call the request_* methods from anywhere you have a Cx, then route every
/// event through handle_event and act on the returned TeslaActions.
/// Requests issued while the access token is stale are queued behind an
/// automatic token refresh.
pub struct TeslaClient {
    creds_path: PathBuf,
    creds: TeslaCredentials,
    region: TeslaRegion,
    refresh_id: Option<LiveId>,
    queued: Vec<PendingCall>,
    in_flight: HashMap<LiveId, PendingCall>,
}

impl TeslaClient {
    /// Loads credentials from a JSON file (see README.md for the schema).
    pub fn load(creds_path: impl Into<PathBuf>) -> Result<Self, TeslaError> {
        let creds_path = creds_path.into();
        let text = std::fs::read_to_string(&creds_path).map_err(|e| {
            TeslaError::Credentials(format!("cannot read {}: {}", creds_path.display(), e))
        })?;
        let creds = TeslaCredentials::deserialize_json_lenient(&text)
            .map_err(|e| TeslaError::Credentials(format!("{}: {:?}", creds_path.display(), e)))?;
        let region = TeslaRegion::from_str(&creds.region).ok_or_else(|| {
            TeslaError::Credentials(format!("region must be na/eu/cn, got '{}'", creds.region))
        })?;
        Ok(Self {
            creds_path,
            creds,
            region,
            refresh_id: None,
            queued: Vec::new(),
            in_flight: HashMap::new(),
        })
    }

    pub fn region(&self) -> TeslaRegion {
        self.region
    }

    pub fn has_in_flight(&self) -> bool {
        !self.in_flight.is_empty() || self.refresh_id.is_some() || !self.queued.is_empty()
    }

    /// GET /api/1/vehicles — list vehicles on the account (vin, name, awake state).
    pub fn request_vehicles(&mut self, cx: &mut Cx) {
        self.submit(cx, ApiCall::Vehicles);
    }

    /// Battery/charging status only — the cheapest useful poll for routing.
    pub fn request_charge_state(&mut self, cx: &mut Cx, vin: &str) {
        self.request_vehicle_data(cx, vin, &[VehicleDataEndpoint::ChargeState]);
    }

    /// Charging status plus the car's own GPS position (needs the
    /// vehicle_location scope on the developer app).
    pub fn request_charge_and_location(&mut self, cx: &mut Cx, vin: &str) {
        self.request_vehicle_data(
            cx,
            vin,
            &[VehicleDataEndpoint::ChargeState, VehicleDataEndpoint::LocationData],
        );
    }

    /// GET /api/1/vehicles/{vin}/vehicle_data with an explicit endpoint set.
    pub fn request_vehicle_data(
        &mut self,
        cx: &mut Cx,
        vin: &str,
        endpoints: &[VehicleDataEndpoint],
    ) {
        let endpoints = endpoints
            .iter()
            .map(|e| e.as_str())
            .collect::<Vec<_>>()
            .join("%3B");
        self.submit(cx, ApiCall::VehicleData { vin: vin.to_string(), endpoints });
    }

    /// POST /api/1/vehicles/{vin}/wake_up. Costs usage credits; the car takes
    /// ~10-30s to come online, poll request_charge_state afterwards.
    pub fn request_wake_up(&mut self, cx: &mut Cx, vin: &str) {
        self.submit(cx, ApiCall::WakeUp { vin: vin.to_string() });
    }

    /// Route all events through this; returns domain actions for the app.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<TeslaAction> {
        let mut out = Vec::new();
        let Event::NetworkResponses(responses) = event else {
            return out;
        };
        for response in responses {
            match response {
                NetworkResponse::HttpResponse { request_id, response } => {
                    if Some(*request_id) == self.refresh_id {
                        self.refresh_id = None;
                        self.handle_token_response(cx, response, &mut out);
                    } else if let Some(pending) = self.in_flight.remove(request_id) {
                        self.handle_api_response(cx, pending, response, &mut out);
                    }
                }
                NetworkResponse::HttpError { request_id, error } => {
                    if Some(*request_id) == self.refresh_id {
                        self.refresh_id = None;
                        self.fail_queue(&mut out, TeslaError::Network(error.message.clone()));
                    } else if self.in_flight.remove(request_id).is_some() {
                        out.push(TeslaAction::Error(TeslaError::Network(error.message.clone())));
                    }
                }
                _ => {}
            }
        }
        out
    }

    // === internals ===

    fn submit(&mut self, cx: &mut Cx, call: ApiCall) {
        self.submit_pending(cx, PendingCall { call, auth_retried: false });
    }

    fn submit_pending(&mut self, cx: &mut Cx, pending: PendingCall) {
        if self.token_is_fresh() {
            self.send_api_call(cx, pending);
        } else {
            self.queued.push(pending);
            self.start_token_refresh(cx);
        }
    }

    fn token_is_fresh(&self) -> bool {
        let Some(expires) = self.creds.access_token_expires else {
            return false;
        };
        self.creds.access_token.is_some() && unix_now() + TOKEN_EXPIRY_MARGIN < expires
    }

    fn start_token_refresh(&mut self, cx: &mut Cx) {
        if self.refresh_id.is_some() {
            return;
        }
        let mut body = format!(
            "grant_type=refresh_token&client_id={}&refresh_token={}",
            form_urlencode(&self.creds.client_id),
            form_urlencode(&self.creds.refresh_token)
        );
        if let Some(secret) = &self.creds.client_secret {
            body.push_str("&client_secret=");
            body.push_str(&form_urlencode(secret));
        }
        let mut request = HttpRequest::new(AUTH_TOKEN_URL.to_string(), HttpMethod::POST);
        request.set_header(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        );
        request.set_string_body(body);
        let request_id = LiveId::unique();
        self.refresh_id = Some(request_id);
        cx.http_request(request_id, request);
    }

    fn send_api_call(&mut self, cx: &mut Cx, pending: PendingCall) {
        let base = self.region.base_url();
        let (url, method) = match &pending.call {
            ApiCall::Vehicles => (format!("{}/api/1/vehicles", base), HttpMethod::GET),
            ApiCall::VehicleData { vin, endpoints } => (
                format!("{}/api/1/vehicles/{}/vehicle_data?endpoints={}", base, vin, endpoints),
                HttpMethod::GET,
            ),
            ApiCall::WakeUp { vin } => {
                (format!("{}/api/1/vehicles/{}/wake_up", base, vin), HttpMethod::POST)
            }
        };
        let mut request = HttpRequest::new(url, method);
        request.set_header(
            "Authorization".to_string(),
            format!("Bearer {}", self.creds.access_token.as_deref().unwrap_or("")),
        );
        request.set_header("Accept".to_string(), "application/json".to_string());
        let request_id = LiveId::unique();
        self.in_flight.insert(request_id, pending);
        cx.http_request(request_id, request);
    }

    fn handle_token_response(
        &mut self,
        cx: &mut Cx,
        response: &HttpResponse,
        out: &mut Vec<TeslaAction>,
    ) {
        let body = response.get_string_body().unwrap_or_default();
        if response.status_code != 200 {
            self.fail_queue(
                out,
                TeslaError::Auth(format!(
                    "token refresh failed, http {}: {}",
                    response.status_code,
                    error_excerpt(&body)
                )),
            );
            return;
        }
        let token = match TokenResponse::deserialize_json_lenient(&body) {
            Ok(t) => t,
            Err(e) => {
                self.fail_queue(out, TeslaError::Parse(format!("token response: {:?}", e)));
                return;
            }
        };
        let Some(access_token) = token.access_token else {
            self.fail_queue(
                out,
                TeslaError::Auth(token.error_description.or(token.error).unwrap_or_else(|| {
                    "token response missing access_token".to_string()
                })),
            );
            return;
        };
        self.creds.access_token = Some(access_token);
        self.creds.access_token_expires = Some(unix_now() + token.expires_in.unwrap_or(28800));
        // Tesla rotates refresh tokens: persist the new one or lose access.
        if let Some(refresh_token) = token.refresh_token {
            self.creds.refresh_token = refresh_token;
        }
        if let Err(e) = std::fs::write(&self.creds_path, self.creds.serialize_json()) {
            out.push(TeslaAction::Error(TeslaError::Credentials(format!(
                "cannot rewrite {}: {} — the rotated refresh token only lives in memory now",
                self.creds_path.display(),
                e
            ))));
        }
        out.push(TeslaAction::TokenRefreshed);
        for pending in std::mem::take(&mut self.queued) {
            self.send_api_call(cx, pending);
        }
    }

    fn handle_api_response(
        &mut self,
        cx: &mut Cx,
        pending: PendingCall,
        response: &HttpResponse,
        out: &mut Vec<TeslaAction>,
    ) {
        let body = response.get_string_body().unwrap_or_default();
        match response.status_code {
            200 => self.parse_api_body(pending, &body, out),
            401 if !pending.auth_retried => {
                // Stale/revoked access token: force a refresh and retry once.
                self.creds.access_token = None;
                self.creds.access_token_expires = None;
                self.submit_pending(cx, PendingCall { auth_retried: true, ..pending });
            }
            408 => {
                if let Some(vin) = pending.call.vin() {
                    out.push(TeslaAction::VehicleAsleep { vin: vin.to_string() });
                } else {
                    out.push(TeslaAction::Error(TeslaError::Http {
                        status: 408,
                        message: error_excerpt(&body),
                    }));
                }
            }
            status => out.push(TeslaAction::Error(TeslaError::Http {
                status,
                message: error_excerpt(&body),
            })),
        }
    }

    fn parse_api_body(&mut self, pending: PendingCall, body: &str, out: &mut Vec<TeslaAction>) {
        match &pending.call {
            ApiCall::Vehicles => match VehiclesResponse::deserialize_json_lenient(body) {
                Ok(parsed) => match parsed.response {
                    Some(vehicles) => out.push(TeslaAction::Vehicles(vehicles)),
                    None => out.push(TeslaAction::Error(TeslaError::Http {
                        status: 200,
                        message: parsed.error.unwrap_or_else(|| "empty vehicle list response".to_string()),
                    })),
                },
                Err(e) => out.push(TeslaAction::Error(TeslaError::Parse(format!("vehicles: {:?}", e)))),
            },
            ApiCall::VehicleData { vin, .. } => {
                match VehicleDataResponse::deserialize_json_lenient(body) {
                    Ok(parsed) => match parsed.response {
                        Some(data) => {
                            let vin = data.vin.clone().unwrap_or_else(|| vin.clone());
                            out.push(TeslaAction::VehicleData { vin, data });
                        }
                        None => out.push(TeslaAction::Error(TeslaError::Http {
                            status: 200,
                            message: parsed
                                .error
                                .unwrap_or_else(|| "empty vehicle_data response".to_string()),
                        })),
                    },
                    Err(e) => out.push(TeslaAction::Error(TeslaError::Parse(format!(
                        "vehicle_data: {:?}",
                        e
                    )))),
                }
            }
            ApiCall::WakeUp { vin } => match WakeUpResponse::deserialize_json_lenient(body) {
                Ok(parsed) => out.push(TeslaAction::WakeUp {
                    vin: vin.clone(),
                    state: parsed.response.and_then(|v| v.state),
                }),
                Err(e) => out.push(TeslaAction::Error(TeslaError::Parse(format!("wake_up: {:?}", e)))),
            },
        }
    }

    fn fail_queue(&mut self, out: &mut Vec<TeslaAction>, error: TeslaError) {
        self.queued.clear();
        out.push(TeslaAction::Error(error));
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn form_urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn error_excerpt(body: &str) -> String {
    if let Ok(parsed) = ErrorBody::deserialize_json_lenient(body) {
        if let Some(error) = parsed.error {
            return match parsed.error_description {
                Some(desc) if !desc.is_empty() => format!("{}: {}", error, desc),
                _ => error,
            };
        }
    }
    let mut excerpt: String = body.chars().take(200).collect();
    if excerpt.is_empty() {
        excerpt.push_str("(empty body)");
    }
    excerpt
}

/// Loads credentials, looking upward from the working directory so apps run
/// from example subdirectories still find the repo-root file.
pub fn load_credentials_search(file_name: &str) -> Result<TeslaClient, TeslaError> {
    let mut dir = std::env::current_dir()
        .map_err(|e| TeslaError::Credentials(format!("current_dir: {}", e)))?;
    loop {
        let candidate = dir.join(file_name);
        if candidate.is_file() {
            return TeslaClient::load(candidate);
        }
        if !dir.pop() {
            return Err(TeslaError::Credentials(format!(
                "{} not found in working directory or any parent — see libs/tesla/README.md",
                file_name
            )));
        }
    }
}

impl TeslaClient {
    /// Convenience: loads `tesla_credentials.json` from the working directory
    /// or any parent (repo root when running via cargo).
    pub fn load_default() -> Result<Self, TeslaError> {
        load_credentials_search("tesla_credentials.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_urlencode_escapes() {
        assert_eq!(form_urlencode("abc-XYZ_0.~"), "abc-XYZ_0.~");
        assert_eq!(form_urlencode("a b+c/d="), "a%20b%2Bc%2Fd%3D");
    }

    #[test]
    fn credentials_roundtrip() {
        let creds = TeslaCredentials {
            client_id: "abcd-1234".to_string(),
            client_secret: None,
            refresh_token: "NA_deadbeef".to_string(),
            region: "eu".to_string(),
            access_token: None,
            access_token_expires: None,
        };
        let json = creds.serialize_json();
        let back = TeslaCredentials::deserialize_json_lenient(&json).unwrap();
        assert_eq!(back.client_id, creds.client_id);
        assert_eq!(back.refresh_token, creds.refresh_token);
        assert_eq!(back.region, "eu");
        assert!(back.access_token.is_none());
    }

    #[test]
    fn token_response_parse() {
        let json = r#"{"access_token":"at","refresh_token":"rt","id_token":"x","expires_in":28800,"token_type":"Bearer"}"#;
        let t = TokenResponse::deserialize_json_lenient(json).unwrap();
        assert_eq!(t.access_token.as_deref(), Some("at"));
        assert_eq!(t.refresh_token.as_deref(), Some("rt"));
        assert_eq!(t.expires_in, Some(28800));
    }
}
