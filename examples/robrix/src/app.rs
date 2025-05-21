use makepad_widgets::*;

live_design!{
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::label::Label;
    // Step 1: Add use statement for AudioPlayer
    import makepad_widgets::audio_player::AudioPlayer;

    App = {{App}} {
        ui: <DesktopWindow>{
            window: {inner_size: vec2(600, 800)},
            show_performance_view: true,
            
            body = <View>{
                flow: Down,
                spacing: 20,
                align: {
                    x: 0.5,
                    y: 0.5
                },
                
                // Welcome label
                <Label> {
                    draw_text:{
                        text_style: <TITLE_TEXT>{font_size: 16},
                        color: #000
                    },
                    text: "Robrix AudioPlayer Demo"
                }

                // Step 2: Add AudioPlayer instance
                audio_player_instance: <AudioPlayer> {}
            }
        }
        
        // Step 3: Configure AudioPlayer properties
        audio_player_instance = {
            source: AudioSource::Url("https://www.soundhelix.com/examples/mp3/SoundHelix-Song-1.mp3"),
            auto_play: true,
            initial_volume: 0.5,
            walk: { width: Fill, height: Fit, margin: {left: 20.0, right: 20.0} },
            layout: {flow: Down, spacing: 10, padding: 10, align: {x:0.5, y:0.0}}
        }
    }
}

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] _app_state: AppState,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            // This is where you'd draw your app UI
            let mut scope = Scope::empty();
            return self.ui.draw_widget(cx, event.clone(), &mut scope);
        }

        // Handle AudioPlayer events if needed by the App logic
        let actions = self.ui.widget_actions(&self.ui.widget_uid());
        for action in actions {
             match action.as_widget_action().cast() {
                AudioPlayerEvent::PlaybackCompleted => {
                    // Example: Log when playback completes
                    log!("Robrix App: AudioPlayer playback completed!");
                }
                AudioPlayerEvent::Error { message } => {
                    log!("Robrix App: AudioPlayer Error: {}", message);
                }
                _ => ()
            }
        }
        
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[derive(Default)]
struct AppState {}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        // Register custom components here if any
    }
}
