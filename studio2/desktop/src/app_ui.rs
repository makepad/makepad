use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let FileTreePane = View {
        width: Fill
        height: Fill
        flow: Down
        file_tree := DesktopFileTree {}
    }

    let CodeEditorPane = View {
        width: Fill
        height: Fill
        flow: Down
        code_editor := DesktopCodeEditor {}
    }

    mod.widgets.AppUI = Window {
        window.inner_size: vec2(1400 900)
        draw_bg +: {
            pixel: fn() {
                return theme.color_bg_app
            }
        }

        body +: {
            width: Fill
            height: Fill
            flow: Down
            spacing: theme.space_2
            padding: theme.space_2

            RoundedView {
                width: Fill
                height: Fit
                flow: Right
                spacing: theme.space_2
                padding: Inset {left: 10.0 right: 10.0 top: 6.0 bottom: 6.0}
                draw_bg.color: #x1B2332
                draw_bg.border_radius: 6.0

                status_label := Label {
                    width: Fit
                    text: "Starting backend..."
                    draw_text.color: #xD5E4FF
                }
                Filler {}
                current_file_label := Label {
                    width: Fit
                    text: "No file"
                    draw_text.color: #x89A0C7
                }
            }

            dock := DockFlat {
                width: Fill
                height: Fill

                root := DockSplitter {
                    axis: SplitterAxis.Horizontal
                    align: SplitterAlign.FromA(310.0)
                    a: @tree_tabs
                    b: @editor_tabs
                }

                tree_tabs := DockTabs {
                    tabs: [@tree_tab]
                    selected: 0
                    closable: false
                }

                editor_tabs := DockTabs {
                    tabs: [@editor_first]
                    selected: 0
                    closable: true
                }

                tree_tab := DockTab {
                    name: "Files"
                    template: @PermanentTab
                    kind: @FileTreePane
                }

                editor_first := DockTab {
                    name: "Editor"
                    template: @PermanentTab
                    kind: @CodeEditorPane
                }

                FileTreePane := FileTreePane {}
                CodeEditorPane := CodeEditorPane {}
            }
        }
    }
}
