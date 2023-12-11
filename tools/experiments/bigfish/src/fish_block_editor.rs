use crate::makepad_widgets::*;

live_design! {
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_widgets::base::*;
    import makepad_draw::shader::std::*;
    import do_not_run_bigfish::fish_theme::*;
    import crate::block_header_button::*;

    FishBlockEditor = <View>
    {
        margin: 30
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
            padding: 0
            draw_bg:
            {
                fn pixel(self) -> vec4
                {
                    return mix(vec4(1,1,0.6,1), vec4(1,1,0.5,1),self.pos.y);
                }
            },
            header = <BlockHeaderButton>
            {

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

            <FishSlider>{text:"Slider!"}


        }
    }

    FishBlockEditorGenerator = <FishBlockEditor>
    {
        title = {draw_bg: { fn pixel(self) -> vec4   { return mix(THEME_COLOR_GENERATOR, THEME_COLOR_GENERATOR_DARK, self.pos.y) }} }
        body = {draw_bg: { fn pixel(self) -> vec4 { return THEME_COLOR_GENERATOR_FADE} }  }
    }

    FishBlockEditorEffect = <FishBlockEditor>
    {
        title = {draw_bg: { fn pixel(self) -> vec4   { return mix(THEME_COLOR_EFFECT, THEME_COLOR_EFFECT_DARK, self.pos.y) }} }
        body = {draw_bg: { fn pixel(self) -> vec4 { return THEME_COLOR_EFFECT_FADE} }  }
    }

    FishBlockEditorMeta = <FishBlockEditor>
    {
        title = {draw_bg: { fn pixel(self) -> vec4   { return mix(THEME_COLOR_META, THEME_COLOR_META_DARK, self.pos.y) }} }
        body = {draw_bg: { fn pixel(self) -> vec4 { return THEME_COLOR_META_FADE} }  }
    }

    FishBlockEditorUtility = <FishBlockEditor>
    {
        title = {draw_bg: { fn pixel(self) -> vec4   { return mix(THEME_COLOR_UTILITY, THEME_COLOR_UTILITY_DARK, self.pos.y) }} }
        body = {draw_bg: { fn pixel(self) -> vec4 { return THEME_COLOR_UTILITY_FADE} }  }
    }

    FishBlockEditorModulator = <FishBlockEditor>
    {
        title = {draw_bg: { fn pixel(self) -> vec4   { return mix(THEME_COLOR_MODULATION, THEME_COLOR_MODULATION_DARK, self.pos.y) }} }
        body = {draw_bg: { fn pixel(self) -> vec4 { return THEME_COLOR_MODULATION_FADE} }  }
    }

    FishBlockEditorEnvelope= <FishBlockEditor>
    {
        title = {draw_bg: { fn pixel(self) -> vec4   { return mix(THEME_COLOR_ENVELOPE, THEME_COLOR_ENVELOPE_DARK, self.pos.y) }} }
        body = {draw_bg: { fn pixel(self) -> vec4 { return THEME_COLOR_ENVELOPE_FADE} }  }
    }
    FishBlockEditorFilter= <FishBlockEditor>
    {
        title = {draw_bg: { fn pixel(self) -> vec4   { return mix(THEME_COLOR_FILTER, THEME_COLOR_FILTER_DARK, self.pos.y) }} }
        body = {draw_bg: { fn pixel(self) -> vec4 { return THEME_COLOR_FILTER_FADE} }  }
    }
}
