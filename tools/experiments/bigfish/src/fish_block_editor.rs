use crate::makepad_widgets::*;

live_design!{

    import makepad_widgets::theme_desktop_dark::*;
    import makepad_widgets::base::*;
    import makepad_draw::shader::std::*;

    FishBlockEditor = <View> 
    {
        
        flow: Down
       // show_bg: true,
       // draw_bg: {
            //fn pixel(self) -> vec4 {
                //return mix(vec4(1,1,0.6,1), vec4(1,1,0.5,1),self.pos.y);
            //}
        //}
        <Label>{text:"Block", draw_text:{color: #0}}
        <View>
        {
            show_bg: true
            flow: Down
            width: Fit
            height: Fit
            padding: 4
           
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(vec4(1,1,0.6,1), vec4(1,1,0.5,1),self.pos.y);
                }
            },
        
           <Label>{text:"SynthBlock", draw_text:{color: #0}}
        }
        <View>
        {
            show_bg: true
            width: Fit
            height: Fit
            flow: Down
            padding: 4
           
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(vec4(1,1,0.9,1), vec4(1,1,0.8,1),self.pos.y);
                }
            }
            <Label>{text:"SynthBlock", draw_text:{color: #0}}
            <Slider>{text:"Slider!"}
        }
      
    }
}