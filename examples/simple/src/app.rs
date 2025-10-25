
use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
        
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                window: {title: "你好，こんにちは, Привет, Hello"},
                body = <View>{
                    flow: Down,
                    spacing:30,
                    align: {
                        x: 0.5,
                        y: 0.5,
                    },
                    <TextInput> {
                        height: 100.0,
                        padding: 20.0,
                        text: r#"Lorem ipsum dolor sit amet, consectetur adipiscing elit. Donec vitae lectus vitae tortor vulputate efficitur. Praesent dictum ultricies odio nec laoreet. Aliquam semper mi at nisl hendrerit fringilla. Donec eleifend tellus a elit congue lacinia. Nam ut nunc eleifend, tincidunt quam vel, feugiat quam. Vestibulum at vulputate ipsum, sed hendrerit elit. Integer metus tortor, interdum id est quis, laoreet semper ipsum. Donec tincidunt enim a nunc suscipit, sollicitudin commodo lacus porttitor.

Nullam volutpat sem ante, vel condimentum metus tincidunt sit amet. Fusce sed accumsan eros. Aliquam erat volutpat. Proin quis sapien et orci malesuada convallis vel ut orci. Fusce cursus ipsum id diam sagittis, at vulputate purus viverra. Praesent pulvinar, turpis sit amet auctor bibendum, quam mi varius massa, et euismod nunc justo nec lorem. Vestibulum tellus lectus, ultrices molestie gravida sit amet, ultrices quis magna. Maecenas a hendrerit felis. In et tellus viverra, ullamcorper odio at, pharetra dui. Phasellus pulvinar augue non aliquam imperdiet. Sed purus ante, finibus ac sodales non, sodales ac dui. Sed porta mauris ante, ac bibendum ante ullamcorper sit amet. Praesent a semper mauris, eget sollicitudin justo. Donec ut efficitur justo, ac bibendum sapien.

Proin non venenatis diam. Aenean interdum urna vitae leo pulvinar, nec cursus nisl rhoncus. Etiam ullamcorper finibus convallis. Quisque eget neque nisi. Maecenas vitae venenatis erat. Donec ac faucibus nisl. In in tempus ipsum. Maecenas vitae arcu auctor, varius arcu vel, efficitur turpis.

Morbi et erat nulla. Lorem ipsum dolor sit amet, consectetur adipiscing elit. Proin congue placerat lacinia. Integer posuere, turpis vel efficitur interdum, urna risus pretium velit, eget tempor metus purus non erat. Phasellus euismod sapien magna, vel vehicula est aliquam sed. Phasellus semper sed mauris et aliquam. Maecenas arcu ex, porta sed tempor et, sodales at elit. Curabitur varius tortor vitae lectus aliquam, quis consequat sapien mattis. Curabitur sed varius ex. Mauris at diam urna. Nam congue fermentum viverra.

Morbi eget urna sit amet ex sollicitudin euismod. Quisque suscipit euismod semper. Mauris mollis velit sapien, vitae porta nisl condimentum et. Maecenas id diam at tellus sagittis auctor. Curabitur placerat molestie nulla, ultricies volutpat libero congue ut. Aenean venenatis, leo in commodo mollis, eros eros blandit odio, eget mollis nunc urna non dolor. Cras molestie aliquet finibus. Suspendisse iaculis posuere nulla in molestie. Nullam lacinia nibh non elit pretium accumsan. Suspendisse eros tortor, auctor nec rhoncus et, dapibus et est. Quisque varius mollis mauris at ornare."#
                    }
                }
            }
        }
    }
}  

app_main!(App); 
 
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] counter: usize,
}
 
impl LiveRegister for App {
    fn live_register(cx: &mut Cx) { 
        crate::makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App{
    fn handle_startup(&mut self, _cx:&mut Cx){
    }
        
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        if self.ui.button(ids!(button_1)).clicked(&actions) {
            self.ui.button(ids!(button_1)).set_text(cx, "Clicked 😀");
            log!("hi");
            self.counter += 1;
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::XrUpdate(_e) = event{
            //log!("{:?}", e.now.left.trigger.analog);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}