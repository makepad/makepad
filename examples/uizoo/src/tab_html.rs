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

            Hr{}
            H4{text: "Plain HTML table (no alignment)"}
            P{text: "Default left-aligned cells. Exercises bold, italic, code, links, sub/sup, emoji, entities, and strikethrough inside cells."}
            Html{
                width: Fill height: Fit
                body: "<table><thead><tr><th>Element</th><th>Symbol</th><th>Notes</th></tr></thead><tbody><tr><td>Hydrogen</td><td><b>H</b></td><td><i>Lightest</i> gas</td></tr><tr><td>Water</td><td>H<sub>2</sub>O</td><td>Covers ~71% of Earth \u{1F30A}</td></tr><tr><td>Carbon-14</td><td><sup>14</sup>C</td><td>Used in <code>dating</code>; see <a href='https://en.wikipedia.org/wiki/Carbon-14'>wiki</a></td></tr><tr><td>Caffeine</td><td>C<sub>8</sub>H<sub>10</sub>N<sub>4</sub>O<sub>2</sub></td><td>\u{2615} keeps you awake</td></tr><tr><td>Specials</td><td>a &amp; b &lt; c</td><td><s>struck</s> / <b>bold</b> / <i>em</i></td></tr></tbody></table>"
            }

            Hr{}
            H4{text: "Aligned HTML table (align attr + style='text-align:')"}
            P{text: "Left / center / right columns set via both the align attribute and inline style. Cells carry mixed formatting, links, sub/sup."}
            Html{
                width: Fill height: Fit
                body: "<table><thead><tr><th align='left'>Task</th><th align='center'>Status</th><th style='text-align: right'>Due</th></tr></thead><tbody><tr><td align='left'>Ship feature <b>X</b></td><td align='center'><code>WIP</code></td><td style='text-align: right'><b>Fri</b></td></tr><tr><td align='left'>Review <a href='https://example.com'>PR #42</a></td><td align='center'><i>Pending</i></td><td style='text-align: right'>Mon</td></tr><tr><td align='left'>Fix <s>critical</s> bug</td><td align='center'>Done \u{2705}</td><td style='text-align: right'>Yesterday</td></tr><tr><td align='left'>Write spec for H<sub>2</sub>O sync</td><td align='center'>50%</td><td style='text-align: right'>2026-05-15</td></tr><tr><td align='left'>Ship Claude<sup>TM</sup> release</td><td align='center'>Blocked</td><td style='text-align: right'>TBD</td></tr></tbody></table>"
            }

            Hr{}
            H4{text: "Collapsible sections (<details> / <summary>)"}
            P{text: "Click the triangle to expand or collapse. The first <details> uses the open attribute to start expanded; the others start collapsed. Nested <details> are supported, and summaries can contain styled text."}
            Html{
                width: Fill height: Fit
                body: "<details open><summary>What is this widget?</summary><p>This is a collapsible section. Each <code>&lt;details&gt;</code> tag wraps a <code>&lt;summary&gt;</code> followed by hidden content that is shown when expanded. Use the <code>open</code> attribute to make it expanded by default.</p></details><details><summary><b>Keyboard shortcuts</b></summary><ul><li><code>Cmd</code> + <code>S</code> — save</li><li><code>Cmd</code> + <code>Z</code> — undo</li><li><code>Cmd</code> + <code>Shift</code> + <code>Z</code> — redo</li></ul></details><details><summary>Nested <i>details</i> with H<sub>2</sub>O inside</summary><p>The outer summary holds a subscript and italics. Inside, you can nest more <code>&lt;details&gt;</code>:</p><details><summary>Level 2: click me</summary><p>Hidden level 2 content. Links still work: <a href='https://example.com'>example.com</a>.</p><details><summary>Level 3: click me too</summary><p>Deeply nested content. Blockquotes work here:</p><blockquote>A quote inside a collapsed-by-default section.</blockquote></details></details></details>"
            }

            Hr{}
            H4{text: "Numeric HTML table (all columns right-aligned)"}
            P{text: "Typical numeric layout using align='right' on every cell. Header, body rows, and a bold totals row."}
            Html{
                width: Fill height: Fit
                body: "<table><thead><tr><th align='left'>Region</th><th align='right'>Q1</th><th align='right'>Q2</th><th align='right'>Q3</th><th align='right'>YoY</th></tr></thead><tbody><tr><td align='left'><b>North America</b></td><td align='right'>$1.2M</td><td align='right'>$1.5M</td><td align='right'>$1.8M</td><td align='right'>+12%</td></tr><tr><td align='left'><i>Europe</i></td><td align='right'>$0.9M</td><td align='right'>$1.1M</td><td align='right'>$1.3M</td><td align='right'>+8%</td></tr><tr><td align='left'>Asia Pacific</td><td align='right'>$0.7M</td><td align='right'>$0.8M</td><td align='right'>$1.0M</td><td align='right'>+15%</td></tr><tr><td align='left'>LATAM</td><td align='right'>$0.2M</td><td align='right'>$0.3M</td><td align='right'>$0.4M</td><td align='right'>+22%</td></tr><tr><td align='left'><b>Total</b></td><td align='right'><b>$3.0M</b></td><td align='right'><b>$3.7M</b></td><td align='right'><b>$4.5M</b></td><td align='right'><b>+13%</b></td></tr></tbody></table>"
            }
        }
    }
}
