//! The two script namespaces every story file relies on and the page
//! templates stories are built from.
//!
//! `mod.stories` holds one template per story, looked up by name at runtime
//! by the canvas. `mod.storybook` holds the shared building blocks so a story
//! file can say `use mod.storybook.*` and write `StoryPage{ StoryRow{ ... } }`.
//! This module registers first; the story files come after it and the app
//! shell last, because a block's `use` only sees what exists when it runs.
use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.storybook = {}
    mod.stories = {}

    /** A story's page: a scrolling column the story fills top to bottom. */
    mod.storybook.StoryPage = View{
        width: Fill
        height: Fill
        flow: Down
        spacing: theme.space_2
        padding: theme.mspace_3
        scroll_bars: ScrollBars{
            show_scroll_x: false
            show_scroll_y: true
            scroll_bar_y.drag_scrolling: true
        }
    }

    /** A row of instances, laid out left to right and centred vertically. */
    mod.storybook.StoryRow = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{x: 0. y: 0.5}
    }

    /** A section heading inside a story page. */
    mod.storybook.StoryHeading = H4{}

    /** A line of explanation under a heading. */
    mod.storybook.StoryNote = P{}

    /** Coloured ground for the glass family to bend.
     *
     * Two kinds of content on purpose. The wash gives the lens something to
     * shift; the rings and the bars give it something to shift AT the scale
     * the refraction works on. A wash alone has no detail that fine, so the
     * bend has nothing to show and the colour split has nothing to split,
     * and the surface reads as a grey rectangle however well it is tuned. */
    mod.storybook.GlassGround = View{
        width: Fill
        height: Fill
        show_bg: true
        draw_bg +: {
            pixel: fn() {
                let p = self.pos
                let h = max(self.rect_size.y, 1.0)
                let aspect = self.rect_size.x / h
                // Square-ish space, so the rings stay round on a wide band.
                let q = vec2(p.x * aspect, p.y)

                let a = vec3(0.05, 0.12, 0.38)
                let b = vec3(0.62, 0.16, 0.42)
                let c = vec3(0.05, 0.42, 0.45)
                let m = mix(a, b, p.x)
                let n = mix(c, b, p.y)
                var col = mix(m, n, 0.45 + 0.35 * sin(p.x * 6.0 + p.y * 3.0))

                let ring_a = abs(length(q - vec2(aspect * 0.26, 0.34)) - 0.21)
                col = mix(vec3(0.98, 0.86, 0.42), col, smoothstep(0.0, 0.014, ring_a))
                let ring_b = abs(length(q - vec2(aspect * 0.72, 0.70)) - 0.28)
                col = mix(vec3(0.42, 0.92, 0.86), col, smoothstep(0.0, 0.014, ring_b))
                let bars = abs(fract((q.x + q.y) * 7.0) - 0.5)
                col = mix(col * 1.35, col, smoothstep(0.0, 0.05, bars - 0.30))
                return vec4(col, 1.0)
            }
        }
    }

    /** A band of ground with a scrim over it and a slot for the demo.
     *
     * Every glass page needs one: these widgets draw what is BEHIND them, so
     * over a plain page there is nothing behind them to draw and the whole
     * family collapses to an outline. The scrim is not decoration either -
     * the ground at full chroma out-shouts the lens, and the refraction is
     * read from the difference between neighbouring pixels, which a scrim
     * keeps legible without flattening the ground into one colour.
     *
     * Every layer is NAMED. A variant of this stage has to override its
     * layers by name: a derived object copies the prototype's children and
     * then appends its own, so an anonymous child in a derived block adds a
     * layer rather than replacing one, and the last thing added in an
     * Overlay flow is painted over everything else. Nothing warns about it.
     *
     * The height is FIXED, never Fit. The ground is `height: Fill`, and a
     * Fill child of a Fit parent is handed nothing at all - which drew no
     * gradient and quietly made this page's whole comparison a lie the first
     * time it was written. Callers set the height; they must not set Fit,
     * and they must leave room for the clearance the body spends. */
    mod.storybook.GlassStage = View{
        width: Fill
        height: 220.
        flow: Overlay

        ground := mod.storybook.GlassGround{}
        // SolidView, not a View with show_bg: a bare View's draw_bg has no
        // `color`, so the wash was declared here for as long as the page
        // existed and never painted a pixel.
        scrim := SolidView{
            width: Fill
            height: Fill
            draw_bg +: {color: #x02040a26}
        }
        body := View{
            width: Fill
            height: Fill
            flow: Down
            spacing: 12.
            // The clearance is arithmetic, not taste. The rim samples along
            // the outward normal by up to `lensing_strength` points, and the
            // panel family ships 28 of them - so ground that stops at the
            // glass hands the bend the page background instead of the
            // ground, and the edge everyone looks at is the part that goes
            // wrong. 30 is that reach, rounded up to the nearest ten, on
            // all four sides: the top and the bottom rims bend exactly as
            // far as the sides do.
            padding: Inset{left: 30., right: 30., top: 30., bottom: 30.}
            align: Align{x: 0. y: 0.5}
        }
    }

    /** The same band with nothing on it at all, for the comparison every
     * glass page should make once: the demo stands over the page itself,
     * which is what a reader who drops one of these on a plain window gets.
     *
     * Both layers are switched off BY NAME. Writing replacements as
     * anonymous children would add a fourth and a fifth layer instead, and
     * whatever was painted last would land over the demo. */
    mod.storybook.FlatStage = mod.storybook.GlassStage{
        ground +: {visible: false}
        scrim +: {visible: false}
    }
}
