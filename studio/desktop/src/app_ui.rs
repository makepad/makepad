use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let STUDIO_HEADER_HEIGHT = 36.0

    let PaneToolbar = RectView {
        width: Fill
        height: STUDIO_HEADER_HEIGHT
        flow: Right
        align: Align {x: 0.0 y: 0.5}
        padding: Inset {left: 8.0 right: 8.0 top: 0.0 bottom: 0.0}
        spacing: theme.space_2
        draw_bg +: {
            color: theme.color_bg_container
        }
    }

    let LogToolbarToggle = Toggle {
        margin: Inset {}
        padding: Inset {left: 0.0 right: 0.0 top: 0.0 bottom: 0.0}
        label_walk: Walk {width: Fit height: Fit margin: Inset {left: 24.0 right: 0.0 top: 0.0 bottom: 0.0}}
        draw_bg +: {
            size: 13.0
        }
        draw_text +: {
            color: theme.color_label_outer_off
            color_hover: theme.color_label_outer
            color_active: theme.color_label_outer
        }
    }

    let SidebarFilterInput = TextInputFlat {
        margin: Inset {}
        padding: Inset {left: 12.0 right: 12.0 top: 0.0 bottom: 0.0}
        draw_bg +: {
            border_radius: 4.0

            color: theme.color_bg_app * 0.82
            color_hover: theme.color_bg_app * 0.88
            color_focus: theme.color_bg_app * 0.92
            color_down: theme.color_bg_app * 0.85
            color_empty: theme.color_bg_app * 0.82

            border_color: theme.color_u_hidden
            border_color_hover: theme.color_u_hidden
            border_color_focus: theme.color_u_hidden
            border_color_down: theme.color_u_hidden
            border_color_empty: theme.color_u_hidden
            border_color_disabled: theme.color_u_hidden

            border_color_2: theme.color_u_hidden
            border_color_2_hover: theme.color_u_hidden
            border_color_2_focus: theme.color_u_hidden
            border_color_2_down: theme.color_u_hidden
            border_color_2_empty: theme.color_u_hidden
            border_color_2_disabled: theme.color_u_hidden
        }
        draw_text +: {
            color_empty: theme.color_label_inner_inactive
            color_empty_hover: theme.color_label_inner_inactive
            color_empty_focus: theme.color_label_outer
        }
    }

    let LogToolbarFilterInput = TextInputFlat {
        margin: Inset {}
        padding: Inset {left: 10.0 right: 10.0 top: 0.0 bottom: 0.0}
        draw_bg +: {
            border_radius: 4.0

            color: theme.color_bg_app * 0.84
            color_hover: theme.color_bg_app * 0.9
            color_focus: theme.color_bg_app * 0.94
            color_down: theme.color_bg_app * 0.87
            color_empty: theme.color_bg_app * 0.84

            border_color: theme.color_u_hidden
            border_color_hover: theme.color_u_hidden
            border_color_focus: theme.color_u_hidden
            border_color_down: theme.color_u_hidden
            border_color_empty: theme.color_u_hidden
            border_color_disabled: theme.color_u_hidden

            border_color_2: theme.color_u_hidden
            border_color_2_hover: theme.color_u_hidden
            border_color_2_focus: theme.color_u_hidden
            border_color_2_down: theme.color_u_hidden
            border_color_2_empty: theme.color_u_hidden
            border_color_2_disabled: theme.color_u_hidden
        }
        draw_text +: {
            color_empty: theme.color_label_inner_inactive
            color_empty_hover: theme.color_label_inner_inactive
            color_empty_focus: theme.color_label_outer
        }
    }

    let LogToolbarButton = ButtonFlatter {
        margin: Inset {}
        padding: Inset {left: 8.0 right: 8.0 top: 0.0 bottom: 0.0}
        draw_text +: {
            color: theme.color_label_outer_off
            color_hover: theme.color_label_outer
            color_down: theme.color_label_outer
            color_focus: theme.color_label_outer
        }
    }

    let LogToolbarIconButton = ButtonFlatterIcon {
        width: 22.0
        height: 22.0
        margin: Inset {}
        icon_walk: Walk {width: 13.0 height: 13.0}
        draw_icon +: {
            color: theme.color_label_outer_off
            color_hover: theme.color_label_outer
            color_down: theme.color_label_outer
            color_focus: theme.color_label_outer
        }
    }

    let FileTreePane = View {
        width: Fill
        height: Fill
        flow: Down
        PaneToolbar {
            file_tree_filter := SidebarFilterInput {
                width: Fill
                empty_text: "Filter"
            }
        }
        file_tree := DesktopFileTree {}
    }

    let CodeEditorPane = View {
        width: Fill
        height: Fill
        flow: Down
        code_editor := DesktopCodeEditor {}
    }

    let EditorFirstPane = RectView {
        draw_bg +: {
            color: theme.color_bg_container
        }
    }

    let RunListPane = View {
        width: Fill
        height: Fill
        flow: Down
        PaneToolbar {
            run_stop_all := ButtonFlat {text: "Stop All"}
        }
        run_list := DesktopRunList {}
    }

    let RunningAppPane = View {
        width: Fill
        height: Fill
        flow: Down
        run_view := DesktopRunView {}
    }

    let RunFirstPane = RectView {
        draw_bg +: {
            color: theme.color_bg_container
        }
        View {
            width: Fill
            height: Fill
            align: Align {x: 0.5 y: 0.5}
            placeholder := Label {
                text: "Click play in Run to launch"
                draw_text.color: theme.color_label_outer
            }
        }
    }

    let LogPane = View {
        width: Fill
        height: Fill
        flow: Down
        PaneToolbar {
            View {
                width: Fit
                height: Fit
                flow: Right
                align: Align {x: 0.0 y: 0.5}
                spacing: theme.space_1

                log_tail_toggle := LogToolbarToggle {
                    text: "Tail"
                    active: true
                }
            }
            Filler {}
            View {
                width: Fit
                height: Fit
                flow: Right
                align: Align {x: 0.0 y: 0.5}
                spacing: 4.0

                log_filter := LogToolbarFilterInput {
                    width: 216.0
                    empty_text: "Filter"
                }
                clear_log_filter := LogToolbarButton {
                    width: 20.0
                    height: 20.0
                    text: "x"
                    padding: Inset {left: 0.0 right: 0.0 top: 0.0 bottom: 0.0}
                }
            }
            View {width: 10.0 height: Fit}
            View {
                width: Fit
                height: Fit
                flow: Right
                align: Align {x: 0.0 y: 0.5}
                spacing: 8.0

                clear_log := LogToolbarButton {
                    text: "Clear"
                }
                log_open_profiler := LogToolbarIconButton {
                    draw_icon +: {
                        svg: crate_resource("self://resources/icons/icon_profiler.svg")
                    }
                }
            }
        }
        log_view := DesktopLogView {}
    }

    let ProfilerPane = View {
        width: Fill
        height: Fill
        flow: Down
        profiler_view := DesktopProfilerView {}
    }

    let LogFirstPane = LogPane {}

    let StudioTerminalView = DesktopTerminalView {
        pad_x: 6.0
        pad_y: 4.0
    }

    let TerminalPane = View {
        width: Fill
        height: Fill
        flow: Down
        terminal_view := StudioTerminalView {}
    }

    let TerminalFirstPane = RectView {
        draw_bg +: {
            color: theme.color_bg_container
        }
        View {
            width: Fill
            height: Fill
            align: Align {x: 0.5 y: 0.5}
            placeholder := Label {
                text: "Terminal press + to add a terminal"
                draw_text.color: theme.color_label_outer
            }
        }
    }

    let TerminalShellPane = View {
        width: Fill
        height: Fill
        terminal_tabs := DockTabs {
            tabs: [@terminal_first @terminal_add]
            selected: 0
            closable: true
        }
    }

    let CaptionChromeToggle = ButtonFlatterIcon {
        width: 36.0
        height: 28.0
        icon_walk: Walk {width: 16.0 height: 16.0}
        draw_bg +: {
            color: #x474747
            color_hover: #x525252
            color_down: #x414141
            border_radius: 4.0
        }
        draw_icon +: {
            color: #xCBCBCB
        }
    }

    let CaptionSidebarToggle = CaptionChromeToggle {
        draw_icon +: {
            svg: crate_resource("self://resources/icons/icon_sidebar_toggle.svg")
        }
    }

    let CaptionPanelToggle = CaptionChromeToggle {
        draw_icon +: {
            svg: crate_resource("self://resources/icons/icon_panel_toggle.svg")
        }
    }

    let STUDIO_PALETTE_1 = #B2FF64
    let STUDIO_PALETTE_2 = #80FFBF
    let STUDIO_PALETTE_3 = #80BFFF
    let STUDIO_PALETTE_4 = #BF80FF
    let STUDIO_PALETTE_5 = #FF80BF
    let STUDIO_PALETTE_6 = #FFB368

    let IconTab = TabFlat {
        closeable: false
        spacing: theme.space_1
        icon_walk: Walk {width: Fit height: 16.0}
        close_button +: {
            width: 11.0
            height: 11.0
            margin: Inset {left: 1.0 right: 7.0 top: 0.0 bottom: 0.0}
            draw_button +: {
                color: #x8C8C8C
                color_hover: #xC8C8C8
                color_active: #xDEDEDE
            }
        }
        draw_text +: {
            color: theme.color_label_inner_inactive
            color_hover: theme.color_label_inner
            color_active: theme.color_label_inner_active
        }
        draw_bg +: {
            color: theme.color_bg_app * 0.84
            color_hover: theme.color_bg_app * 0.96
            color_active: theme.color_fg_app

            border_color: theme.color_u_hidden
            border_color_hover: theme.color_u_hidden
            border_color_active: theme.color_bg_app * 0.92

            border_color_2: theme.color_u_hidden
            border_color_2_hover: theme.color_u_hidden
            border_color_2_active: theme.color_bg_app * 0.92
        }
    }

    let MountTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_3
            svg: crate_resource("self://resources/icons/icon_tab_app.svg")
        }
    }

    let FilesTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_2
            svg: crate_resource("self://resources/icons/icon_file.svg")
        }
    }

    let RunListTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_5
            svg: crate_resource("self://resources/icons/icon_run.svg")
        }
    }

    let EditorFirstTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_6
            svg: crate_resource("self://resources/icons/icon_editor.svg")
        }
    }

    let EditorTab = EditorFirstTab {closeable: true}

    let RunFirstTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_4
            svg: crate_resource("self://resources/icons/icon_tab_app.svg")
        }
    }

    let RunAppTab = RunFirstTab {closeable: true}

    let LogFirstTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_2
            svg: crate_resource("self://resources/icons/icon_log.svg")
        }
    }

    let LogTab = LogFirstTab {closeable: true}

    let TerminalTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_2
            svg: crate_resource("self://resources/icons/icon_terminal.svg")
        }
    }

    let TerminalCloseableTab = TabFlat {
        closeable: true
        spacing: theme.space_1
        draw_text +: {
            color: theme.color_label_inner_inactive
            color_hover: theme.color_label_inner
            color_active: theme.color_label_inner_active
        }
        draw_bg +: {
            color: theme.color_bg_app * 0.84
            color_hover: theme.color_bg_app * 0.95
            color_active: theme.color_fg_app

            border_color: theme.color_u_hidden
            border_color_hover: theme.color_u_hidden
            border_color_active: theme.color_bg_app * 0.92

            border_color_2: theme.color_u_hidden
            border_color_2_hover: theme.color_u_hidden
            border_color_2_active: theme.color_bg_app * 0.92
        }
        close_button +: {
            width: 11.0
            height: 11.0
            margin: Inset {left: 1.0 right: 7.0 top: 0.0 bottom: 0.0}
            draw_button +: {
                color: #x8C8C8C
                color_hover: #xC8C8C8
                color_active: #xDEDEDE
            }
        }
    }

    let TerminalAddTab = TabFlat {
        closeable: false
        width: 28.0
        spacing: 0.0
        align: Center
        padding: Inset {left: 0.0 right: 0.0 top: theme.space_2 bottom: theme.space_2}
        icon_walk: Walk {width: 0.0 height: 0.0}
        draw_text +: {
            color: theme.color_label_inner_inactive
            color_hover: theme.color_label_inner
            color_active: theme.color_label_inner_active
            text_style: theme.font_bold{
                font_size: theme.font_size_p + 1.0
            }
        }
        draw_bg +: {
            color: theme.color_bg_app * 0.82
            color_hover: theme.color_bg_app * 0.94
            color_active: theme.color_fg_app

            border_color: theme.color_u_hidden
            border_color_hover: theme.color_u_hidden
            border_color_active: theme.color_bg_app * 0.92

            border_color_2: theme.color_u_hidden
            border_color_2_hover: theme.color_u_hidden
            border_color_2_active: theme.color_bg_app * 0.92
        }
    }

    let StudioDock = DockFlat {
        tab_bar +: {
            height: STUDIO_HEADER_HEIGHT
        }
        splitter +: {
            draw_bg +: {
                color: theme.color_bg_container
                color_hover: theme.color_bevel_outset_1_hover * 0.45
                color_drag: theme.color_bevel_outset_1_hover * 0.7
                border_radius: 1.5
                splitter_pad: 1.5
            }
        }
    }

    mod.widgets.AppUI = Window {
        window.inner_size: vec2(1400 900)
        caption_bar := SolidView {
            visible: true
            height: STUDIO_HEADER_HEIGHT
            flow: Right
            align: Align {x: 0.0 y: 0.5}
            draw_bg.color: theme.color_bg_app

            left_controls := View {
                width: Fit
                height: Fit
                flow: Right
                align: Align {x: 0.0 y: 0.5}
                margin: Inset {left: 72.0 right: 0.0 top: 0.0 bottom: 0.0}

                sidebar_toggle := CaptionSidebarToggle {}
            }

            caption_label := View {
                width: Fill
                height: Fill
                align: Center
                label := Label {
                    text: "Makepad"
                    padding: 0.0
                    draw_text +: {
                        color: theme.color_label_outer
                        text_style: theme.font_bold{
                            font_size: theme.font_size_p + 0.5
                        }
                    }
                }
            }

            right_caption_tools := View {
                width: Fit
                height: Fit
                flow: Right
                spacing: theme.space_1
                margin: Inset {left: 0.0 right: 96.0 top: 0.0 bottom: 0.0}

                bottom_panel_toggle := CaptionPanelToggle {}
                voice_wave := VoiceWave {
                    width: Fit
                    height: Fit
                }
            }

            windows_buttons := View {
                visible: false
                width: Fit
                height: Fit
                flow: Right
                align: Align {x: 0.0 y: 0.5}
                min := DesktopButton {draw_bg.button_type: DesktopButtonType.WindowsMin width: 46 height: 29}
                max := DesktopButton {draw_bg.button_type: DesktopButtonType.WindowsMax width: 46 height: 29}
                close := DesktopButton {draw_bg.button_type: DesktopButtonType.WindowsClose width: 46 height: 29}
            }

            web_fullscreen := View {
                visible: false
                width: Fit
                height: Fit
                align: Align {x: 0.0 y: 0.5}
                margin: Inset {left: 0.0 right: 8.0 top: 0.0 bottom: 0.0}
                fullscreen := DesktopButton {draw_bg.button_type: DesktopButtonType.Fullscreen width: 50 height: 36}
            }
        }
        draw_bg +: {
            pixel: fn() {
                return theme.color_bg_app
            }
        }

        body +: {
            width: Fill
            height: Fill
            flow: Down
            spacing: 0.0
            padding: Inset {left: 10.0 right: 10.0 top: 2.0 bottom: 10.0}

            RoundedView {
                visible: false
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

            mount_dock := StudioDock {
                width: Fill
                height: Fill

                tab_bar +: {
                    MountTab := MountTab {}
                }

                root := DockTabs {
                    tabs: [@mount_first]
                    selected: 0
                    closable: false
                }

                mount_first := DockTab {
                    name: "makepad"
                    template: @MountTab
                    kind: @MountWorkspace
                }

                MountWorkspace := View {
                    width: Fill
                    height: Fill

                    dock := StudioDock {
                        width: Fill
                        height: Fill

                        tab_bar +: {
                            FilesTab := FilesTab {}
                            RunListTab := RunListTab {}
                            EditorFirstTab := EditorFirstTab {}
                            EditorTab := EditorTab {}
                            RunFirstTab := RunFirstTab {}
                            RunAppTab := RunAppTab {}
                            LogFirstTab := LogFirstTab {}
                            LogTab := LogTab {}
                            TerminalTab := TerminalTab {}
                            TerminalCloseableTab := TerminalCloseableTab {}
                            TerminalAddTab := TerminalAddTab {}
                        }

                        root := DockSplitter {
                            axis: SplitterAxis.Horizontal
                            align: SplitterAlign.FromA(310.0)
                            a: @tree_tabs
                            b: @main_split
                        }

                        main_split := DockSplitter {
                            axis: SplitterAxis.Vertical
                            align: SplitterAlign.FromB(220.0)
                            a: @editor_split
                            b: @bottom_panel_tabs
                        }

                        editor_split := DockSplitter {
                            axis: SplitterAxis.Horizontal
                            align: SplitterAlign.Weighted(0.62)
                            a: @editor_tabs
                            b: @run_tabs
                        }

                        bottom_panel_tabs := DockTabs {
                            tabs: [@log_first @bottom_terminal_tab]
                            selected: 0
                            closable: false
                        }

                        tree_tabs := DockTabs {
                            tabs: [@tree_tab @run_list_tab]
                            selected: 0
                            closable: false
                        }

                        editor_tabs := DockTabs {
                            tabs: [@editor_first]
                            selected: 0
                            closable: true
                        }

                        run_tabs := DockTabs {
                            tabs: [@run_first]
                            selected: 0
                            closable: true
                        }

                        tree_tab := DockTab {
                            name: "Files"
                            template: @FilesTab
                            kind: @FileTreePane
                        }

                        run_list_tab := DockTab {
                            name: "Run"
                            template: @RunListTab
                            kind: @RunListPane
                        }

                        editor_first := DockTab {
                            name: ""
                            template: @EditorFirstTab
                            kind: @EditorFirstPane
                        }

                        run_first := DockTab {
                            name: ""
                            template: @RunFirstTab
                            kind: @RunFirstPane
                        }

                        log_first := DockTab {
                            name: "Logs"
                            template: @LogFirstTab
                            kind: @LogFirstPane
                        }

                        terminal_first := DockTab {
                            name: ""
                            template: @TerminalTab
                            kind: @TerminalFirstPane
                        }

                        terminal_add := DockTab {
                            name: "+"
                            template: @TerminalAddTab
                            kind: @TerminalAddPane
                        }

                        bottom_terminal_tab := DockTab {
                            name: "Terminal"
                            template: @TerminalTab
                            kind: @TerminalShellPane
                        }

                        FileTreePane := FileTreePane {}
                        RunListPane := RunListPane {}
                        CodeEditorPane := CodeEditorPane {}
                        EditorFirstPane := EditorFirstPane {}
                        RunningAppPane := RunningAppPane {}
                        RunFirstPane := RunFirstPane {}
                        LogFirstPane := LogFirstPane {}
                        LogPane := LogPane {}
                        ProfilerPane := ProfilerPane {}
                        TerminalFirstPane := TerminalFirstPane {}
                        TerminalShellPane := TerminalShellPane {}
                        TerminalPane := TerminalPane {}
                        TerminalAddPane := View {}
                    }
                }
            }
        }
    }
}
