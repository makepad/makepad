pub mod backend;
// The blocking client is a native socket API; wasm uses the async web backend.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod blocking_http;
pub mod digest;
// The embedded TCP server is native-only; browsers cannot listen on sockets.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod http_server;
// The blocking TCP websocket is native-only; wasm uses backend::web.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod plain_web_socket;
pub mod runtime;
pub mod socket_stream;
pub mod types;
pub mod ui_signal;
// TCP parsing helpers expose native socket deadlines alongside pure parsers.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod utils;
pub mod web_socket_parser;

pub const HTTP_BODY_LIMIT_ERROR: &str = "response body exceeds configured limit";

pub use crate::backend::{EventSink, NetworkBackend, UnsupportedBackend};
pub use crate::http_server::{
    start_http_server, HttpServer, HttpServerRequest, HttpServerResponse,
    HttpServerResponseSender,
};
pub use crate::runtime::{NetworkConfig, NetworkRuntime};
pub use crate::socket_stream::SocketStream;
pub use crate::types::{
    HttpError, HttpMethod, HttpProgress, HttpRequest, HttpResponse, NetworkError, NetworkResponse,
    SplitUrl, WebSocketMessage, WebSocketTransport, WsMessage, WsSend,
};
pub use crate::ui_signal::{
    install_ui_waker, to_ui_bounded, to_ui_oneshot, FromUIReceiver, FromUISender,
    ReceiverAlreadyTaken, SignalFromUI, SignalToUI, ToUIOneshotReceiver, ToUIOneshotSender,
    ToUIReceiver, ToUISender, UiWaker,
};
pub use crate::utils::HttpServerHeaders;
pub use crate::web_socket_parser::{
    ServerWebSocketError, ServerWebSocketMessage, ServerWebSocketMessageFormat,
    ServerWebSocketMessageHeader, WebSocketError, WebSocketMessage as ParsedWebSocketMessage,
    WebSocketMessageFormat, WebSocketMessageHeader, WebSocketParser,
    SERVER_WEB_SOCKET_PING_MESSAGE, SERVER_WEB_SOCKET_PONG_MESSAGE,
};
pub use makepad_error_log;

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
