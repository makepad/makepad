use crate::makepad_widgets::*;

live_design! {

    use makepad_widgets::theme_desktop_dark::*;
    use makepad_widgets::base::*;
    use makepad_draw::shader::std::*;
    use makepad_audio_widgets::piano::Piano;
    BigFishHomeScreen=  <View> {
        flow:Down
        <View>
        {
        width: Fill,
        height: Fill,
        flow: Down
        align:
        {
            x:0.5,
            y:0.5
        }
        <Label>{text:"Welcome!"
        margin: 40
            draw_text: {
                color: #f,
                text_style: {

                    font_size: 20.0,
                    height_factor: 1.0,

                    font: {path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Regular.ttf")}
                },
            }
        }
        <Image> {
        source: dep("crate://self/resources/colourfish.png"),
        width: (431*0.5 ), height: (287*0.5), margin: { top: 0.0, right: 0.0, bottom: 0.0, left: 10.0  }

        }
    }
    <View>
    {
        height: Fit,
        align:{x:0.5, y:1.0}
        <Piano> {height: Fit, width: Fill, margin:0}
    }

    }
}
