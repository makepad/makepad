/// Canvas command sent from WS/HTTP thread to Makepad UI thread
#[derive(Debug, Clone)]
pub enum CanvasCommand {
    /// Splash code to render (replace entire panel)
    SplashRender {
        code: String,
    },
    /// Splash streaming: begin / append / end
    SplashStreamBegin,
    SplashStreamAppend {
        code: String,
    },
    SplashStreamEnd,
    /// Evaluate Splash expression in current context
    SplashEval {
        code: String,
    },
    /// Audio playback commands
    AudioPlay { url: String },
    AudioPause,
    AudioStop,
    AudioToggle,
    /// Save current app with a name
    SaveApp { name: String },
    /// CLI connected/disconnected
    ConnectionState {
        connected: bool,
    },
}
