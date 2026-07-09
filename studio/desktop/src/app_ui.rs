use crate::makepad_widgets::*;

#[path = "app_ui/ai_pane.rs"]
pub mod ai_pane;
#[path = "app_ui/bottom_panes.rs"]
pub mod bottom_panes;
#[path = "app_ui/editor_panes.rs"]
pub mod editor_panes;
#[path = "app_ui/shared.rs"]
pub mod shared;
#[path = "app_ui/sidebar_panes.rs"]
pub mod sidebar_panes;

pub fn register_all(vm: &mut ScriptVm) {
    shared::script_mod(vm);
    ai_pane::script_mod(vm);
    sidebar_panes::script_mod(vm);
    editor_panes::script_mod(vm);
    bottom_panes::script_mod(vm);
    self::script_mod(vm);
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let STUDIO_HEADER_HEIGHT = 36.0

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

    let BottomBarIconButton = ButtonFlatterIcon {
        width: 38.0
        height: 26.0
        margin: Inset {}
        icon_walk: Walk {width: 16.0 height: 16.0}
        draw_bg +: {
            color: theme.color_u_hidden
            color_hover: theme.color_bg_highlight
            color_down: theme.color_bg_highlight * 0.78
            color_focus: theme.color_u_hidden
            border_radius: 4.0
        }
        draw_icon +: {
            color: theme.color_label_outer
            color_hover: theme.color_label_outer_hover
            color_down: theme.color_label_inner_active
            color_focus: theme.color_label_outer
        }
    }

    let StudioBottomBar = SolidView {
        width: Fill
        height: 30.0
        flow: Right
        align: Align {x: 0.0 y: 0.5}
        padding: Inset {left: 5.0 right: 5.0 top: 0.0 bottom: 0.0}
        spacing: 4.0
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                let color = vec4(theme.color_bg_container.rgb, 0.12)
                let highlight = 0.02 * smoothstep(1.5, 0.0, self.pos.y * self.rect_size.y)
                let noise = (Math.random_2d(self.pos * 1000.0) - 0.5) * 0.005
                sdf.fill(vec4(color.rgb + highlight + noise, color.a))

                // Top separator line
                let thickness = 1.0
                if self.pos.y * self.rect_size.y <= thickness {
                    sdf.clear(vec4(1.0, 1.0, 1.0, 0.05))
                }
                return sdf.result
            }
        }

        let BottomBarSeparator = View {
            width: 1.0
            height: 14.0
            margin: Inset {left: 2.0 right: 2.0}
            show_bg: true
            draw_bg +: {
                color: vec4(1.0, 1.0, 1.0, 0.08)
            }
        }

        bottom_file_tree_toggle := BottomBarIconButton {
            draw_icon.svg: crate_resource("self://resources/icons/icon_file.svg")
        }
        BottomBarSeparator {}
        bottom_run_list_toggle := BottomBarIconButton {
            draw_icon.svg: crate_resource("self://resources/icons/icon_run.svg")
        }
        BottomBarSeparator {}
        bottom_panel_toggle := BottomBarIconButton {
            draw_icon.svg: crate_resource("self://resources/icons/icon_panel_toggle.svg")
        }
        bottom_bar_spacer := View {
            width: Fill
            height: Fill
        }
        BottomBarSeparator {}
        bottom_agent_toggle := BottomBarIconButton {
            draw_icon.svg: crate_resource("self://resources/icons/icon_ai.svg")
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
            color: vec4(1.0, 1.0, 1.0, 0.0)
            color_hover: vec4(1.0, 1.0, 1.0, 0.04)
            color_active: vec4(1.0, 1.0, 1.0, 0.08)

            border_color: theme.color_u_hidden
            border_color_hover: theme.color_u_hidden
            border_color_active: vec4(1.0, 1.0, 1.0, 0.12)

            border_color_2: theme.color_u_hidden
            border_color_2_hover: theme.color_u_hidden
            border_color_2_active: vec4(1.0, 1.0, 1.0, 0.12)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                sdf.box_y(
                    self.border_size + self.overlap_fix
                    self.border_size
                    self.rect_size.x - self.border_size * 2. - self.overlap_fix
                    self.rect_size.y
                    self.border_radius
                    max(self.border_size * 0.5, 0.5)
                )

                let fill = self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_active, self.active)

                let stroke = self.border_color
                    .mix(self.border_color_hover, self.hover)
                    .mix(self.border_color_active, self.active)

                sdf.fill_keep(fill)
                sdf.stroke(stroke, self.border_size)

                let accent_thickness = 1.5
                let accent_color = vec4(0.38, 0.69, 0.91, 1.0)
                if self.active > 0.5 && self.pos.y * self.rect_size.y >= self.rect_size.y - accent_thickness {
                    return accent_color
                }

                return sdf.result
            }
        }
    }

    let MountTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_3
            svg: crate_resource("self://resources/icons/icon_tab_app.svg")
        }
    }

    let AiTab = IconTab {
        draw_icon +: {
            color: STUDIO_PALETTE_1
            svg: crate_resource("self://resources/icons/icon_ai.svg")
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
            color: vec4(1.0, 1.0, 1.0, 0.0)
            color_hover: vec4(1.0, 1.0, 1.0, 0.04)
            color_active: vec4(1.0, 1.0, 1.0, 0.08)

            border_color: theme.color_u_hidden
            border_color_hover: theme.color_u_hidden
            border_color_active: vec4(1.0, 1.0, 1.0, 0.12)

            border_color_2: theme.color_u_hidden
            border_color_2_hover: theme.color_u_hidden
            border_color_2_active: vec4(1.0, 1.0, 1.0, 0.12)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                sdf.box_y(
                    self.border_size + self.overlap_fix
                    self.border_size
                    self.rect_size.x - self.border_size * 2. - self.overlap_fix
                    self.rect_size.y
                    self.border_radius
                    max(self.border_size * 0.5, 0.5)
                )

                let fill = self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_active, self.active)

                let stroke = self.border_color
                    .mix(self.border_color_hover, self.hover)
                    .mix(self.border_color_active, self.active)

                sdf.fill_keep(fill)
                sdf.stroke(stroke, self.border_size)

                let accent_thickness = 1.5
                let accent_color = vec4(0.38, 0.69, 0.91, 1.0)
                if self.active > 0.5 && self.pos.y * self.rect_size.y >= self.rect_size.y - accent_thickness {
                    return accent_color
                }

                return sdf.result
            }
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

    let StudioDock = DockFlat {
        tab_bar +: {
            height: STUDIO_HEADER_HEIGHT
            draw_bg +: {
                color: vec4(0.0, 0.0, 0.0, 0.0)
            }
            CloseableTab := mod.widgets.TabFlat {
                closeable: true
                spacing: theme.space_1
                draw_text +: {
                    color: theme.color_label_inner_inactive
                    color_hover: theme.color_label_inner
                    color_active: theme.color_label_inner_active
                }
                draw_bg +: {
                    color: vec4(1.0, 1.0, 1.0, 0.0)
                    color_hover: vec4(1.0, 1.0, 1.0, 0.04)
                    color_active: vec4(1.0, 1.0, 1.0, 0.08)

                    border_color: theme.color_u_hidden
                    border_color_hover: theme.color_u_hidden
                    border_color_active: vec4(1.0, 1.0, 1.0, 0.12)

                    border_color_2: theme.color_u_hidden
                    border_color_2_hover: theme.color_u_hidden
                    border_color_2_active: vec4(1.0, 1.0, 1.0, 0.12)

                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                        sdf.box_y(
                            self.border_size + self.overlap_fix
                            self.border_size
                            self.rect_size.x - self.border_size * 2. - self.overlap_fix
                            self.rect_size.y
                            self.border_radius
                            max(self.border_size * 0.5, 0.5)
                        )

                        let fill = self.color
                            .mix(self.color_hover, self.hover)
                            .mix(self.color_active, self.active)

                        let stroke = self.border_color
                            .mix(self.border_color_hover, self.hover)
                            .mix(self.border_color_active, self.active)

                        sdf.fill_keep(fill)
                        sdf.stroke(stroke, self.border_size)

                        let accent_thickness = 1.5
                        let accent_color = vec4(0.38, 0.69, 0.91, 1.0)
                        if self.active > 0.5 && self.pos.y * self.rect_size.y >= self.rect_size.y - accent_thickness {
                            return accent_color
                          }

                        return sdf.result
                    }
                }
            }
            PermanentTab := mod.widgets.TabFlat {
                closeable: false
                spacing: theme.space_1
                draw_text +: {
                    color: theme.color_label_inner_inactive
                    color_hover: theme.color_label_inner
                    color_active: theme.color_label_inner_active
                }
                draw_bg +: {
                    color: vec4(1.0, 1.0, 1.0, 0.0)
                    color_hover: vec4(1.0, 1.0, 1.0, 0.04)
                    color_active: vec4(1.0, 1.0, 1.0, 0.08)

                    border_color: theme.color_u_hidden
                    border_color_hover: theme.color_u_hidden
                    border_color_active: vec4(1.0, 1.0, 1.0, 0.12)

                    border_color_2: theme.color_u_hidden
                    border_color_2_hover: theme.color_u_hidden
                    border_color_2_active: vec4(1.0, 1.0, 1.0, 0.12)

                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                        sdf.box_y(
                            self.border_size + self.overlap_fix
                            self.border_size
                            self.rect_size.x - self.border_size * 2. - self.overlap_fix
                            self.rect_size.y
                            self.border_radius
                            max(self.border_size * 0.5, 0.5)
                        )

                        let fill = self.color
                            .mix(self.color_hover, self.hover)
                            .mix(self.color_active, self.active)

                        let stroke = self.border_color
                            .mix(self.border_color_hover, self.hover)
                            .mix(self.border_color_active, self.active)

                        sdf.fill_keep(fill)
                        sdf.stroke(stroke, self.border_size)

                        let accent_thickness = 1.5
                        let accent_color = vec4(0.38, 0.69, 0.91, 1.0)
                        if self.active > 0.5 && self.pos.y * self.rect_size.y >= self.rect_size.y - accent_thickness {
                            return accent_color
                        }

                        return sdf.result
                    }
                }
            }
        }
        splitter +: {
            draw_bg +: {
                color: vec4(1.0, 1.0, 1.0, 0.05)
                color_hover: vec4(1.0, 1.0, 1.0, 0.20)
                color_drag: vec4(1.0, 1.0, 1.0, 0.45)
                border_radius: 1.5
                splitter_pad: 1.5
            }
        }
    }

    mod.widgets.AppUI = Window {
        pass +: { clear_color: #00000000 }
        window.inner_size: vec2(1400 900)
        caption_bar := SolidView {
            visible: true
            height: STUDIO_HEADER_HEIGHT
            flow: Right
            align: Align {x: 0.0 y: 0.5}
            draw_bg +: {
                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                    let color = vec4(theme.color_bg_app.rgb, 0.12)
                    let highlight = 0.03 * smoothstep(1.5, 0.0, self.pos.y * self.rect_size.y)
                    let noise = (Math.random_2d(self.pos * 1000.0) - 0.5) * 0.005
                    sdf.fill(vec4(color.rgb + highlight + noise, color.a))

                    // Bottom separator line
                    let thickness = 1.0
                    if self.pos.y * self.rect_size.y >= self.rect_size.y - thickness {
                        sdf.clear(vec4(1.0, 1.0, 1.0, 0.06))
                    }
                    return sdf.result
                }
            }

            left_controls := View {
                visible: false
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
                let color = vec4(theme.color_bg_app.rgb, 0.55)
                let highlight = 0.04 * smoothstep(2.0, 0.0, self.pos.y * self.rect_size.y)
                let noise = (Math.random_2d(self.pos * 1000.0) - 0.5) * 0.008
                return vec4(color.rgb + highlight + noise, color.a)
            }
        }

        body +: {
            width: Fill
            height: Fill
            flow: Down
            spacing: 0.0
            padding: Inset {}

            main_work_area := View {
                width: Fill
                height: Fill
                margin: Inset {left: 10.0 right: 10.0 top: 2.0 bottom: 0.0}
                flow: Down
                spacing: 0.0

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
                                AiTab := AiTab {}
                                EditorFirstTab := EditorFirstTab {}
                                EditorTab := EditorTab {}
                                RunFirstTab := RunFirstTab {}
                                RunAppTab := RunAppTab {}
                                LogFirstTab := LogFirstTab {}
                                LogTab := LogTab {}
                                TerminalTab := TerminalTab {}
                                TerminalCloseableTab := TerminalCloseableTab {}
                            }

                            root := DockSplitter {
                                axis: SplitterAxis.Horizontal
                                align: SplitterAlign.FromA(310.0)
                                a: @tree_tabs
                                b: @agent_split
                            }

                            agent_split := DockSplitter {
                                axis: SplitterAxis.Horizontal
                                align: SplitterAlign.FromB(310.0)
                                a: @main_split
                                b: @agent_tabs
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
                                tabs: [@log_first @terminal_first]
                                selected: 0
                                closable: false
                            }

                            tree_tabs := DockTabs {
                                tabs: [@tree_tab @run_list_tab]
                                selected: 0
                                closable: false
                                hide_tab_bar: true
                            }

                            agent_tabs := DockTabs {
                                tabs: [@ai_tab]
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

                            ai_tab := DockTab {
                                name: "AI"
                                template: @AiTab
                                kind: @AiPane
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
                                name: "Terminal"
                                template: @TerminalTab
                                kind: @TerminalFirstPane
                            }

                            FileTreePane := FileTreePane {}
                            RunListPane := RunListPane {}
                            AiPane := AiPane {}
                            CodeEditorPane := CodeEditorPane {}
                            EditorFirstPane := EditorFirstPane {}
                            RunningAppPane := RunningAppPane {}
                            RunFirstPane := RunFirstPane {}
                            LogFirstPane := LogFirstPane {}
                            LogPane := LogPane {}
                            ProfilerPane := ProfilerPane {}
                            TerminalFirstPane := TerminalFirstPane {}
                            TerminalPane := TerminalPane {}
                        }
                    }
                }
            }

            StudioBottomBar {}
        }
    }
}
