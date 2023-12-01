use crate::makepad_widgets::*;

live_design!{
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_widgets::base::*;
    import makepad_draw::shader::std::*;
    import do_not_run_bigfish::fish_theme::*;
   

    FishBlockEditor = <View> 
    {
        margin: 20
        width: 200
        height: Fit
        flow: Down
        optimize: DrawList
      
        title = <View>
        {
            show_bg: true
            flow: Down
            width: Fill
            height: Fit
            padding: 4
            draw_bg: 
            {
                fn pixel(self) -> vec4 
                {
                    return mix(vec4(1,1,0.6,1), vec4(1,1,0.5,1),self.pos.y);
                }
            },
            <Label>
            {
                text:"Synth Block", 
                draw_text:
                {
                        color: #0
                        text_style: <H2_TEXT_BOLD> {}
                }
            }
        }
        body = <View>
        {
            show_bg: true
            width: Fill
            height: Fit
            flow: Down
            padding: 4
           
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(vec4(1,1,0.9,1), vec4(1,1,0.8,1),self.pos.y);
                }
            }
            <Label>{text:"Synth Block", draw_text:{color: #0, text_style: <H2_TEXT_REGULAR>{}}}
            <FishSlider>{text:"Slider!"}
            <FishSlider>{text:"Slider!"}
            <FishSlider>{text:"Slider!"}
            <FishSlider>{text:"Slider!"}
        }
      
    }
    FishBlockEditorGenerator = <FishBlockEditor>
    {

    }

    FishBlockEditorEffect = <FishBlockEditor>
    {

    }

    FishBlockEditorMeta = <FishBlockEditor>
    {
        title = {
            draw_bg: {
                
                    fn pixel(self) -> vec4 
                    {
                        return THEME_COLOR_META
                    }
                }
            }
        
    }

    FishBlockEditorUtility = <FishBlockEditor>
    {

    }

    FishBlockEditorModulator = <FishBlockEditor>
    {

    }

    FishBlockEditorEnvelope= <FishBlockEditor>
    {

    }
    FishBlockEditorFilter= <FishBlockEditor>
    {

    }
}