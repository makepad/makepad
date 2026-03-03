pub mod backend;
pub mod digest;
pub mod http_server;
pub mod plain_web_socket;
pub mod runtime;
pub mod socket_stream;
pub mod types;
pub mod ui_signal;
pub mod utils;
pub mod web_socket_parser;

pub use crate::backend::{EventSink, NetworkBackend, UnsupportedBackend};
pub use crate::http_server::{
    start_http_server, HttpServer, HttpServerRequest, HttpServerResponse,
};
pub use crate::runtime::{NetworkConfig, NetworkRuntime};
pub use crate::socket_stream::SocketStream;
pub use crate::types::{
    HttpError, HttpMethod, HttpProgress, HttpRequest, HttpResponse, NetworkError, NetworkResponse,
    SplitUrl, WebSocketMessage, WebSocketTransport, WsMessage, WsSend,
};
pub use crate::ui_signal::{
    FromUIReceiver, FromUISender, SignalFromUI, SignalToUI, ToUIReceiver, ToUISender,
};
pub use crate::utils::HttpServerHeaders;
pub use crate::web_socket_parser::{
    ServerWebSocketError, ServerWebSocketMessage, ServerWebSocketMessageFormat,
    ServerWebSocketMessageHeader, WebSocketError, WebSocketMessage as ParsedWebSocketMessage,
    WebSocketMessageFormat, WebSocketMessageHeader, WebSocketParser,
    SERVER_WEB_SOCKET_PING_MESSAGE, SERVER_WEB_SOCKET_PONG_MESSAGE,
};

#[cfg(target_os = "android")]
pub use crate::backend::{
    clear_platform_backend as clear_android_backend_shim,
    clear_platform_socket_factory as clear_android_socket_stream_factory_shim,
    register_platform_backend as register_android_backend_shim,
    register_platform_socket_factory as register_android_socket_stream_factory_shim,
    PlatformSocketFactory as AndroidSocketStreamFactory,
    PlatformSocketStream as AndroidSocketStream,
};

#[cfg(target_arch = "wasm32")]
pub use crate::backend::web::{
    clear_platform_backend as clear_wasm_backend_shim,
    register_platform_backend as register_wasm_backend_shim,
};
