//! The markdown story: a block of every construct the widget draws.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.stories.MarkdownOverview = StoryPage{
        Markdown{
            width: Fill height: Fit
            body: "# Headline 1 \n ## Headline 2 \n ### Headline 3 \n #### Headline 4 \n This is standard text with a  \n\n line break a short ~~strike through~~ demo.\n\n *Italic text* \n\n **Bold text** \n\n - Bullet\n - Another bullet\n - Third bullet\n 1. Numbered list Bullet\n 2. Another list entry\n 3. Third list entry\n `Monospaced text`\n> This is a quote.\nThis is `inline code`.\n ```code block```"
        }

        Hr{}
        H4{text: "Tables"}
        P{text: "A pipe table lays out through the text flow's own table code, the same code an html table element uses. Plain, aligned and numeric tables are shown once, on the Html page; the separator row's colons set a column's alignment here the way the align attribute does there."}
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "text/textflow/markdown",
    category: "Text",
    component: "TextFlow",
    also: &["Markdown", "MarkdownLink"],
    name: "Markdown",
    dsl: "MarkdownOverview",
    added: "2026-04-18",
    tags: &["ported"],
    doc: "# Markdown\n\n`Markdown` lays out a `body` written in markdown through `TextFlow`: headings, emphasis, bullet and numbered lists, quotes, inline code, fenced code blocks, links and pipe tables.\n\nThe parser runs with tables and maths switched on and strike through off, so `~~struck~~` shows as written, tildes and all, as it does in the block on this page.\n\nEvery block it opens starts on a line of its own, because the flow underneath keeps laying out where it left off until the host breaks the line. A link is a `MarkdownLink`, a widget the flow places inline with the words either side of it.\n\nTables are the flow's too. `:---`, `:---:` and `---:` in the separator row align a column left, centre or right, and the three tables on the Html page show the same layout.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];
