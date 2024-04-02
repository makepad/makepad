use crate::{
    makepad_widgets::*,
}; 
 
live_design!{ 
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    App = {{App}} {
        
        ui: <Window> {
            show_bg: true
            width: Fill,
            height: Fill
            body = {
                <SlidesView> {
                    <Slide> {
                        title = {text: " Hello Exec(ut)"},
                        <SlideBody> {text: ""}
                    }
                    <SlideChapter> {
                        title = {text: "Playing with local AI"},
                        <SlideBody> {text: "Rik Arends\n"}
                    }
                    <Slide> {
                        title = {text:"Makepad"},
                        <SlideBody> {text: "- Rebuilding VB/Delphi\n   in Rust"}
                    }                    
                    <Slide> {
                        title = {text: "10 years is a long time"},
                        <SlideBody> {text: "- Whole stack from scratch"}
                    }
                    <Slide> {
                        title = {text: "AI moves incredibly fast"},
                        <SlideBody> {text: "- september 2023\n- 2 months later: 6x faster"}
                    }
                    <Slide> {
                        title = {text: "Large Language Models"},
                        <SlideBody> {text: "- text input, text output\n- GPT 3.5, LLama, Mistral"}
                    }
                    <Slide> {
                        title = {text: "Diffuser models"},
                        <SlideBody> {text: "- text input, image output\n- Midjourney, Stable Diffusion, Dall-E"}
                    }
                    <Slide> {
                        title = {text: "Multi modal"},
                        <SlideBody> {text: "- Can use image/audio as inputs"}
                    }
                    <Slide> {
                        title = {text: "Lets build!"},
                        <SlideBody> {text: "- a generative image explorer\n- conversational input\n- image output"}
                    }
                    <Slide> {
                        title = {text: "Lets go local"},
                        <SlideBody> {text: "- Fun, it's your own computer\n- Can be faster\n- Privacy"}
                    }
                    <Slide> {
                        title = {text: "Language model"},
                        <SlideBody> {text: "- Mixtral 8x7b\n- Very close to chatGPT"}
                    }
                    <Slide> {
                        title = {text: "Image model"},
                        <SlideBody> {text: "- SDXL Turbo\n- Fast and good"}
                    }
                    <Slide> {
                        title = {text: "How much compute"},
                        <SlideBody> {text: "- GPU 1: NVidia 4090 for SDXL\n- 86 TFLOP\n- GPU 2: Apple M3 max for Mixtral\n- 28 TFLOP: 114\n- 2005 2nd BGW Blue gene\n- 448KW, 41000 cores"}
                    }
                    <Slide> {
                        title = {text: "Lets try it"},
                        <SlideBody> {text: "- Pure prompt\n- Conversation\n- Camera input"}
                    }
                    <Slide> {
                        title = {text: "Demo ComfyUI"},
                    }
                    <Slide> {
                        title = {text: "Llama CPP"},
                    }
                    <Slide> {
                        title = {text: "Future"},
                        <SlideBody> {text: "- Hold on to your hats\n- Integrate LLM in code editor"}
                    }
                    <Slide> {
                        title = {text: "Links"}, 
                        <SlideBody> {text: "- github.com/makepad/makepad\n- makepad.nl\n- twitter @rikarends"}
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

}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
