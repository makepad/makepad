use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.DemoFT = UIZooTabLayout_B{
        desc +: {
            Markdown{body: "# FileTree\n\nFileTree displays a file system tree."}
        }
        demos +: {
            DemoFileTree{file_tree +: {width: Fill height: Fill}}
        }
    }
}
