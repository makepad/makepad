use crate::makepad_widgets::*;

live_design! {
    use makepad_widgets::theme_desktop_dark::*;
    use makepad_widgets::base::*;
    use makepad_draw::shader::std::*;
    use crate::fish_theme::*;
    use crate::block_header_button::*;
    use crate::block_delete_button::*;

    FishBlockEditor = <View>
    {
        margin: 0
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
            topbar = <View>
            {
                flow:Right,
                height: Fit,
                header = <BlockHeaderButton>
                {
                    draw_text:
                    {
                        color: #0
                        text_style: <H2_TEXT_BOLD> {}
                    }
                }
                delete = <BlockDeleteButton>
                {
                    width: Fit,
                    draw_text:
                    {
                        color: #0
                        text_style: <H2_TEXT_BOLD> {}
                    }
                }
                padding = <View>
                {
                    width: 20
                }
            }
        }
        body = <View>
        {
            show_bg: true
            width: Fill
            height: Fit
            flow: Down
            padding: {left: 30, right: 30, top: 4, bottom: 4}

            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(vec4(1,1,0.9,1), vec4(1,1,0.8,1),self.pos.y);
                }
            }

            <FishSlider>{text:"Slider A"}
            <FishSlider>{text:"Slider B"}
            <FishSlider>{text:"Slider C"}

        }
    }

    FishBlockEditorGenerator = <FishBlockEditor>
    {
        title = {draw_bg: { fn pixel(self) -> vec4 { return mix(THEME_COLOR_GENERATOR, THEME_COLOR_GENERATOR_DARK, self.pos.y) }} }
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
