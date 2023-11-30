use crate::makepad_widgets::*;

live_design!{

    import makepad_widgets::theme_desktop_dark::*;
    import makepad_widgets::base::*;
    import makepad_draw::shader::std::*;

    BigFishHomeScreen = <View> {
        

       
        

        width: Fill,
        height: Fill,
        flow: Down
        
       <Label>{text:"Welcome to BigFish!", draw_text:{color: #f}}
       <Image> {
        source: dep("crate://self/resources/tinrs_mobile.png"),
        width: (178 * 0.175), height: (121 * 0.175), margin: { top: 0.0, right: 0.0, bottom: 0.0, left: 10.0  }
       
   }
        <Button>{text:"wtf"}
    }
}