use crate::{
    makepad_widgets::*,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use crate::layout_templates::*;

    BoxA = <RoundedView> {
        height: Fill, width: Fill,
        padding: 10., margin: 0.,
        show_bg: true,
        draw_bg: { color: #0F02 }
        flow: Down,
        align: { x: 0.5, y: 0.5}
        <P> { width: Fit, text: "width: Fill\nheight: Fill\nflow: Down"}
    }

    BoxB = <RoundedView> {
        height: Fit, width: Fit,
        padding: 10., margin: 0.,
        show_bg: true,
        draw_bg: { color: #F002 }
        flow: Down,
        align: { x: 0.5, y: 0.5}
        <P> { width: Fit, text: "width: Fit\nheight: Fit\nflow: Down"}
    }

    BoxC = <RoundedView> {
        height: 200, width: Fill,
        padding: 10., margin: 0.,
        spacing: 10.
        show_bg: true,
        draw_bg: { color: #0002 }
        flow: Right,
        align: { x: 0.5, y: 0.5}
        <P> { width: Fit, text: "width: Fill\nheight: 200\nflow: Right,\n spacing: 10."}
    }



    pub DemoLayout = <UIZooTabLayout_B> {
        desc = {
            <H3> { text: "LayoutDemos"}
            <Markdown> {
                body: "
                The `Layout` struct controls the layout properties of elements, such as positioning, alignment, spacing, padding, and scrolling.
                ## Fields
                ### align
                Specifies how child elements are aligned within the parent container along the horizontal (`x`) and vertical (`y`) axes.
                - `x` (f64 = 0.0): Horizontal alignment. A value between `0.0` (left) and `1.0` (right).
                - `y` (f64 = 0.0): Vertical alignment. A value between `0.0` (top) and `1.0` (bottom).
                ### clip_x (bool = true)
                Enables or disables horizontal clipping. When `true`, content that extends beyond the container's horizontal bounds is clipped.
                ### clip_y (bool = true)
                Enables or disables vertical clipping. When `true`, content that extends beyond the container's vertical bounds is clipped.
                ### flow (`Flow::Right`)
                Determines the layout direction of child elements within the container.
                Possible values:
                - `Flow::Right`: Arranges child elements horizontally from left to right.
                - `Flow::Down`: Arranges child elements vertically from top to bottom.
                - `Flow::Overlay`: Stacks child elements on top of each other.
                - `Flow::RightWrap`: Arranges child elements horizontally, wrapping to the next line when the right edge is reached.
                ### line_spacing (f64 = 0.0)
                Specifies the spacing between lines when wrapping content or arranging elements in a vertical flow.
                ### padding
                Sets the internal padding of the container, controlling the distance between the container's borders and its content.
                #### Padding Fields
                - `left` (f64 = 0.0): Padding on the left side.
                - `top` (f64 = 0.0): Padding on the top side.
                - `right` (f64 = 0.0): Padding on the right side.
                - `bottom` (f64 = 0.0): Padding on the bottom side.
                ### scroll (DVec2 = vec2(0.0, 0.0))
                Sets the scroll offset of the content within the container.
                ### spacing (f64 = 0.0)
                Specifies the spacing between child elements within the container.
                ### Size
                Defines how elements consume space within their parent container.
                #### Size Variants
                - `Size::Fixed(f64)`: Sets the element to a fixed size.
                - `Size::Fill`: The element fills the remaining available space in the parent container, accounting for padding and margins.
                - `Size::Fit`: The element sizes itself to fit its content.
                - `Size::All`: The element takes up all available space in the parent container, ignoring any padding and margins.
                "
            }
        }
        demos = {
            <BoxA> {}
            <BoxB> {}
            <BoxB> {}
            <BoxC> {
                <BoxB> {}
                <BoxB> {}
                <BoxA> {}
                <Filler> {}
                <BoxB> {}
            }
        }
    }
}