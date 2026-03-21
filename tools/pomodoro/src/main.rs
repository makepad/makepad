use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

// ── Pomodoro State ──

#[derive(Clone)]
enum Mode { Work, Break, LongBreak }

struct Pomodoro {
    mode: Mode,
    remaining: u32,
    total: u32,
    running: bool,
    sessions: u32,
}

impl Pomodoro {
    fn new() -> Self {
        Self { mode: Mode::Work, remaining: 1500, total: 1500, running: false, sessions: 0 }
    }

    fn toggle(&mut self) { self.running = !self.running; }

    fn reset(&mut self) {
        self.running = false;
        self.remaining = self.mode_duration();
        self.total = self.remaining;
    }

    fn skip(&mut self) {
        self.next_mode();
    }

    fn tick(&mut self) -> bool {
        if !self.running || self.remaining == 0 { return false; }
        self.remaining -= 1;
        if self.remaining == 0 {
            self.next_mode();
            return true; // completed
        }
        false
    }

    fn next_mode(&mut self) {
        match self.mode {
            Mode::Work => {
                self.sessions += 1;
                self.mode = if self.sessions >= 4 {
                    self.sessions = 0;
                    Mode::LongBreak
                } else {
                    Mode::Break
                };
            }
            Mode::Break | Mode::LongBreak => {
                self.mode = Mode::Work;
            }
        }
        self.running = false;
        self.remaining = self.mode_duration();
        self.total = self.remaining;
    }

    fn mode_duration(&self) -> u32 {
        match self.mode { Mode::Work => 1500, Mode::Break => 300, Mode::LongBreak => 900 }
    }

    fn to_splash(&self) -> String {
        let m = self.remaining / 60;
        let s = self.remaining % 60;
        let time_str = format!("{m:02}:{s:02}");
        let pct = if self.total > 0 { (self.total - self.remaining) * 100 / self.total } else { 0 };
        let bar_w = std::cmp::max(1, 400 * pct / 100);

        let (label, color) = match self.mode {
            Mode::Work => ("FOCUS TIME", "#xff6b6b"),
            Mode::Break => ("SHORT BREAK", "#x51cf66"),
            Mode::LongBreak => ("LONG BREAK", "#x339af0"),
        };
        let (btn_text, btn_color) = if self.running { ("Pause", "#xffa94d") } else { ("Start", "#x51cf66") };

        let mut dots = String::new();
        for i in 0..4u32 {
            if i > 0 { dots.push(' '); }
            dots.push_str(if i < self.sessions { "\u{1f345}" } else { "\u{26aa}" });
        }

        format!(
            "SolidView{{width: Fill height: Fit draw_bg.color: #x0a0a12 flow: Down align: Center spacing: 20 \
            padding: Inset{{left: 40. right: 40. top: 50. bottom: 40.}} \
            View{{width: Fit height: Fit flow: Right spacing: 8 \
            Label{{text: \"{dots}\" draw_text.text_style.font_size: 16}}}} \
            Label{{text: \"{label}\" draw_text.color: {color} draw_text.text_style.font_size: 14}} \
            Label{{text: \"{time_str}\" draw_text.color: #xffffff draw_text.text_style.font_size: 64}} \
            View{{width: 400 height: 8 flow: Overlay \
            RoundedView{{width: Fill height: Fill draw_bg.color: #x222233 draw_bg.radius: 4.}} \
            RoundedView{{width: {bar_w} height: Fill draw_bg.color: {color} draw_bg.radius: 4.}}}} \
            Label{{text: \"{pct}%\" draw_text.color: #x666688 draw_text.text_style.font_size: 11}} \
            View{{height: 20}} \
            View{{width: Fit height: Fit flow: Right spacing: 16 align: Center \
            start_btn := Button{{text: \"{btn_text}\" draw_bg.color: {btn_color} draw_text.color: #x111111 \
            padding: Inset{{left: 24. right: 24. top: 12. bottom: 12.}} draw_bg.radius: 6.}} \
            reset_btn := Button{{text: \"Reset\" draw_bg.color: #x444466 draw_text.color: #xccccdd \
            padding: Inset{{left: 24. right: 24. top: 12. bottom: 12.}} draw_bg.radius: 6.}} \
            skip_btn := Button{{text: \"Skip\" draw_bg.color: #x333355 draw_text.color: #x9999bb \
            padding: Inset{{left: 24. right: 24. top: 12. bottom: 12.}} draw_bg.radius: 6.}}}} \
            View{{height: 10}} \
            View{{width: Fit height: Fit flow: Right spacing: 20 \
            Label{{text: \"\u{1f345} 25min\" draw_text.color: #xff6b6b draw_text.text_style.font_size: 11}} \
            Label{{text: \"\u{2615} 5min\" draw_text.color: #x51cf66 draw_text.text_style.font_size: 11}} \
            Label{{text: \"\u{1f334} 15min\" draw_text.color: #x339af0 draw_text.text_style.font_size: 11}}}}}}"
        )
    }
}

fn play_sound(sound: &str) {
    let _ = std::process::Command::new("afplay")
        .arg(format!("/System/Library/Sounds/{sound}.aiff"))
        .spawn();
}

// ── Main ──

#[tokio::main]
async fn main() {
    let port = std::fs::read_to_string("/tmp/makepad-canvas.port")
        .expect("Canvas not running — start makepad-canvas first")
        .trim()
        .to_string();

    let url = format!("ws://127.0.0.1:{port}");
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("Failed to connect to Canvas");

    eprintln!("🍅 Pomodoro connected to Canvas (port {port})");

    let (mut ws_write, mut ws_read) = ws.split();
    let mut pomo = Pomodoro::new();

    // Send initial render
    let msg = serde_json::json!({"splash": pomo.to_splash()});
    ws_write.send(Message::Text(msg.to_string())).await.unwrap();

    let mut tick_interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            // 1-second tick
            _ = tick_interval.tick() => {
                if pomo.running {
                    let completed = pomo.tick();
                    if completed {
                        play_sound("Glass");
                    } else if pomo.remaining <= 5 && pomo.remaining > 0 {
                        play_sound("Tink");
                    }
                    let msg = serde_json::json!({"splash": pomo.to_splash()});
                    if ws_write.send(Message::Text(msg.to_string())).await.is_err() {
                        eprintln!("Canvas disconnected");
                        break;
                    }
                }
            }

            // Events from Canvas (button clicks)
            Some(msg) = ws_read.next() => {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
                            match data.get("widget").and_then(|v| v.as_str()) {
                                Some("start_btn") => {
                                    pomo.toggle();
                                    if pomo.running { play_sound("Pop"); }
                                }
                                Some("reset_btn") => {
                                    pomo.reset();
                                    play_sound("Pop");
                                }
                                Some("skip_btn") => {
                                    pomo.skip();
                                }
                                _ => {}
                            }
                            let msg = serde_json::json!({"splash": pomo.to_splash()});
                            if ws_write.send(Message::Text(msg.to_string())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => {
                        eprintln!("Canvas connection closed");
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
}
