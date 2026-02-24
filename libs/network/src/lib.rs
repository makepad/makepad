pub mod backend;
pub mod digest;
pub mod http_server;
pub mod plain_web_socket;
pub mod runtime;
pub mod types;
pub mod utils;
pub mod web_socket_parser;

pub use crate::backend::{EventSink, NetworkBackend, UnsupportedBackend};
pub use crate::http_server::{start_http_server, HttpServer, HttpServerRequest, HttpServerResponse};
pub use crate::runtime::{NetworkConfig, NetworkRuntime};
pub use crate::types::{
    HttpError, HttpMethod, HttpProgress, HttpRequest, HttpResponse, NetworkError,
    NetworkResponse, SplitUrl, WebSocketMessage, WebSocketTransport, WsMessage, WsSend,
};
pub use crate::utils::HttpServerHeaders;
pub use crate::web_socket_parser::{
    ServerWebSocketError, ServerWebSocketMessage, ServerWebSocketMessageFormat,
    ServerWebSocketMessageHeader, WebSocketParser, SERVER_WEB_SOCKET_PING_MESSAGE,
    SERVER_WEB_SOCKET_PONG_MESSAGE,
};

#[cfg(target_os = "android")]
pub use crate::backend::android::{
    clear_platform_backend as clear_android_backend_shim,
    register_platform_backend as register_android_backend_shim,
};

#[cfg(target_arch = "wasm32")]
pub use crate::backend::web::{
    clear_platform_backend as clear_wasm_backend_shim,
    register_platform_backend as register_wasm_backend_shim,
};
