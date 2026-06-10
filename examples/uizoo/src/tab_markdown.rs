use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoMarkdown = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# Markdown\n\nThe Markdown widget renders markdown content."}
        }
        demos +: {
            Markdown{
                width: Fill height: Fit
                body: "# Headline 1 \n ## Headline 2 \n ### Headline 3 \n #### Headline 4 \n This is standard text with a  \n\n line break a short ~~strike through~~ demo.\n\n *Italic text* \n\n **Bold text** \n\n - Bullet\n - Another bullet\n - Third bullet\n 1. Numbered list Bullet\n 2. Another list entry\n 3. Third list entry\n `Monospaced text`\n> This is a quote.\nThis is `inline code`.\n ```code block```"
            }

            Hr{}
            H4{text: "Plain Markdown table (no alignment)"}
            P{text: "Default left-aligned cells. Exercises bold, italic, code, links, sub/sup, emoji, and entities inside cells."}
            Markdown{
                width: Fill height: Fit
                body: "| Element | Symbol | Notes |\n| --- | --- | --- |\n| Hydrogen | **H** | *Lightest* gas |\n| Water | H<sub>2</sub>O | Covers ~71% of Earth \u{1F30A} |\n| Carbon-14 | <sup>14</sup>C | Used in `dating`; see [wiki](https://en.wikipedia.org/wiki/Carbon-14) |\n| Caffeine | C<sub>8</sub>H<sub>10</sub>N<sub>4</sub>O<sub>2</sub> | \u{2615} keeps you awake |\n| Specials | a &amp; b &lt; c | ~~struck~~ / **bold** / *em* |"
            }

            Hr{}
            H4{text: "Aligned Markdown table (left / center / right)"}
            P{text: "Column alignments set by the separator row. Mixes wide and narrow cells with formatting, links, and sub/sup."}
            Markdown{
                width: Fill height: Fit
                body: "| Task | Status | Due |\n|:-----|:------:|----:|\n| Ship feature **X** | `WIP` | **Fri** |\n| Review [PR #42](https://example.com) | *Pending* | Mon |\n| Fix ~~critical~~ bug | Done \u{2705} | Yesterday |\n| Write spec for H<sub>2</sub>O sync | 50% | 2026-05-15 |\n| Ship Claude<sup>TM</sup> release | Blocked | TBD |"
            }

            Hr{}
            H4{text: "Numeric Markdown table (right-aligned)"}
            P{text: "Typical use-case: every column right-aligned for numeric data. Wide-ish column widths expose the row-by-row alignment."}
            Markdown{
                width: Fill height: Fit
                body: "| Region | Q1 | Q2 | Q3 | YoY |\n| ---:| ---:| ---:| ---:| ---:|\n| **North America** | $1.2M | $1.5M | $1.8M | +12% |\n| *Europe* | $0.9M | $1.1M | $1.3M | +8% |\n| Asia Pacific | $0.7M | $0.8M | $1.0M | +15% |\n| LATAM | $0.2M | $0.3M | $0.4M | +22% |\n| **Total** | **$3.0M** | **$3.7M** | **$4.5M** | **+13%** |"
            }
        }
    }
}
