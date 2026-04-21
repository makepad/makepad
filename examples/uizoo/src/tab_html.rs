use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoHtml = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# Html\n\nThe Html widget renders HTML content."}
        }
        demos +: {
            Html{
                width: Fill height: Fit
                body: "<H1>H1 Headline</H1><H2>H2 Headline</H2><H3>H3 Headline</H3><H4>H4 Headline</H4><H5>H5 Headline</H5><H6>H6 Headline</H6>This is <b>bold</b>&nbsp;and <i>italic text</i>.<sep><b><i>Bold italic</i></b>, <u>underlined</u>, and <s>strike through</s> text. <p>This is a paragraph</p> <code>A code block</code>. <br/> And this is a <a href='https://www.google.com/'>link</a><br/><ul><li>lorem</li><li>ipsum</li><li>dolor</li></ul><ol><li>lorem</li><li>ipsum</li><li>dolor</li></ol><br/> <blockquote>Blockquote</blockquote> <pre>pre</pre><sub>sub</sub><del>del</del>"
            }

            Hr{}
            H4{text: "Html with ellipsis (1 line)"}
            P{text: "Html content truncated to a single line. Resize the window to see the ellipsis move."}
            Html{
                width: Fill height: Fit
                max_lines: 1
                text_overflow: Ellipsis
                body: "This is <b>bold</b> and <i>italic</i> and <code>inline code</code> and regular text that goes on long enough to be truncated with an ellipsis at the end of the line."
            }

            Hr{}
            H4{text: "Html with ellipsis (2 lines)"}
            P{text: "Styled Html wrapping to 2 lines before truncating."}
            Html{
                width: Fill height: Fit
                max_lines: 2
                text_overflow: Ellipsis
                body: "The <b>quick brown fox</b> jumps over the <i>lazy dog</i>. Pack my box with <b><i>five dozen</i></b> liquor jugs. How <u>vexingly quick</u> daft zebras jump! The five boxing wizards jump quickly. Sphinx of black quartz, judge my vow. Two driven jocks help fax my big quiz."
            }

            Hr{}
            H4{text: "Html with ellipsis and emoji (1 line)"}
            P{text: "Html with multi-byte emoji mixed into styled text."}
            Html{
                width: Fill height: Fit
                max_lines: 1
                text_overflow: Ellipsis
                body: "Stars \u{2B50}\u{2B50}\u{2B50} with <b>bold rockets \u{1F680}\u{1F680}\u{1F680}</b> and <i>italic flags \u{1F3C1}\u{1F3C1}\u{1F3C1}</i> and more content to overflow"
            }
        }
    }
}
