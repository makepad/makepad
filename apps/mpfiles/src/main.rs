//! mpfiles — the file browser of the mp* desktop.
//!
//! A GNOME-Files-shaped browser: tabs, a places-and-bookmarks sidebar, an
//! editable breadcrumb path bar, and four views over one folder (icons with
//! real thumbnails, a sortable DataGrid list with expandable folders, a
//! compact list, and a treemap of where the bytes actually are). Space quick-
//! looks the selection the way macOS does; inside mpwm the compositor hosts
//! that popup for us.
//!
//! Everything here is the shell — the entry model lives in `model`, the views
//! in `contents`, thumbnails in `thumbs`, file operations in `ops`, the
//! treemap's arithmetic in `treemap` and its widget in `treemap_view`.

pub use makepad_widgets;

use makepad_widgets::makepad_platform::thread::SignalToUI;
use makepad_widgets::*;

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
};

mod bookmarks;
mod contents;
mod demo;
mod menu;
mod model;
mod ops;
mod preview;
mod rename;
mod theme;
mod thumbs;
mod treemap;
mod treemap_view;
mod vfs;

use crate::{
    bookmarks::Bookmarks,
    contents::{FileContents, FileContentsAction, ViewMode, DEFAULT_ZOOM, ZOOM_LEVELS},
    model::{display_name, trash_dir, FileEntry},
    menu::{MenuAction, MenuRow},
    ops::{Journal, OpKind, OpRequest, OpUpdate, Ops},
    preview::{Preview, PreviewHost},
    rename::BatchMode,
    theme::Palette,
    vfs::vfs,
};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let ToolButton = View{
        width: 28
        height: 28
        flow: Overlay
        align: Align{x: 0.5 y: 0.5}
        cursor: MouseCursor.Hand
        btn_sel := SolidView{
            visible: false
            width: Fill
            height: Fill
            draw_bg +: {color: mod.mpf.sel}
        }
    }

    let SideItem = SolidView{
        width: Fill
        height: 32
        flow: Right
        spacing: 12
        padding: Inset{left: 18 right: 12}
        align: Align{y: 0.5}
        cursor: MouseCursor.Hand
        draw_bg +: {color: mod.mpf.bg_dark}
        side_icon := Icon{
            icon_walk: Walk{width: 16 height: 16}
            draw_icon +: {color: mod.mpf.fg_dim}
        }
        side_title := Label{
            width: Fill
            max_lines: 1
            text_overflow: TextOverflow.Ellipsis
            draw_text +: {
                color: mod.mpf.fg
                text_style: theme.font_regular{font_size: 10.0}
            }
        }
    }

    // A bookmark is a place with a remove button that appears under the
    // pointer — the same shape GNOME's sidebar uses.
    let BookmarkItem = SolidView{
        visible: false
        width: Fill
        height: 32
        flow: Right
        spacing: 12
        padding: Inset{left: 18 right: 8}
        align: Align{y: 0.5}
        cursor: MouseCursor.Hand
        draw_bg +: {color: mod.mpf.bg_dark}
        bm_icon := Icon{
            icon_walk: Walk{width: 13 height: 13}
            draw_icon +: {
                svg: crate_resource("self://resources/icons/bookmark.svg")
                color: mod.mpf.fg_dim
            }
        }
        bm_title := Label{
            width: Fill
            max_lines: 1
            text_overflow: TextOverflow.Ellipsis
            draw_text +: {
                color: mod.mpf.fg
                text_style: theme.font_regular{font_size: 10.0}
            }
        }
        bm_remove := View{
            visible: false
            width: 18
            height: 18
            align: Align{x: 0.5 y: 0.5}
            cursor: MouseCursor.Hand
            Icon{
                icon_walk: Walk{width: 9 height: 9}
                draw_icon +: {
                    svg: crate_resource("self://resources/icons/close.svg")
                    color: mod.mpf.fg_dim
                }
            }
        }
    }

    let SectionLabel = Label{
        height: 26
        padding: Inset{left: 18 top: 8}
        draw_text +: {
            color: mod.mpf.fg_dim
            text_style: theme.font_bold{font_size: 8.0}
        }
    }

    let Divider = View{
        width: Fill
        height: 13
        align: Align{y: 0.5}
        SolidView{
            width: Fill
            height: 1
            margin: Inset{left: 16 right: 16}
            draw_bg +: {color: mod.mpf.muted}
        }
    }

    let MenuRow = View{
        width: Fill
        height: 28
        flow: Right
        spacing: 8
        padding: Inset{left: 14 right: 14}
        align: Align{y: 0.5}
        cursor: MouseCursor.Hand
        // A tick is an icon like every other mark in this app: the UI font
        // draws U+2713 as something closer to a radical sign.
        menu_check := View{
            visible: false
            width: 10
            height: 10
            align: Align{x: 0.5 y: 0.5}
            Icon{
                icon_walk: Walk{width: 9 height: 9}
                draw_icon +: {
                    svg: crate_resource("self://resources/icons/check.svg")
                    color: mod.mpf.accent
                }
            }
        }
        menu_gap := View{
            width: 10
            height: 1
        }
        menu_label := Label{
            width: Fill
            draw_text +: {
                color: mod.mpf.fg
                text_style: theme.font_regular{font_size: 10.0}
            }
        }
    }

    let Crumb = View{
        width: Fit
        height: 24
        padding: Inset{left: 8 right: 8}
        align: Align{y: 0.5}
        cursor: MouseCursor.Hand
        crumb_title := Label{
            max_lines: 1
            draw_text +: {
                color: mod.mpf.fg
                text_style: theme.font_regular{font_size: 10.0}
            }
        }
    }

    let CrumbSep = Label{
        text: "›"
        draw_text +: {
            color: mod.mpf.fg_dim
            text_style: theme.font_regular{font_size: 10.0}
        }
    }

    // One tab. Square, flat, filled when active — the strip only appears once
    // there is more than one of them.
    let TabItem = SolidView{
        visible: false
        width: 172
        height: Fill
        flow: Right
        spacing: 4
        padding: Inset{left: 12 right: 6}
        align: Align{y: 0.5}
        cursor: MouseCursor.Hand
        draw_bg +: {color: mod.mpf.bg_dark}
        tab_title := Label{
            width: Fill
            max_lines: 1
            text_overflow: TextOverflow.Ellipsis
            draw_text +: {
                color: mod.mpf.fg
                text_style: theme.font_regular{font_size: 9.5}
            }
        }
        tab_close := View{
            width: 18
            height: 18
            align: Align{x: 0.5 y: 0.5}
            cursor: MouseCursor.Hand
            Icon{
                icon_walk: Walk{width: 9 height: 9}
                draw_icon +: {
                    svg: crate_resource("self://resources/icons/close.svg")
                    color: mod.mpf.fg_dim
                }
            }
        }
    }

    // One line of the properties panel: a quiet key over its value.
    let PropRow = View{
        width: Fill
        height: Fit
        flow: Down
        spacing: 2
        padding: Inset{left: 16 right: 16 top: 7 bottom: 7}
        prop_key := Label{
            draw_text +: {
                color: mod.mpf.fg_dim
                text_style: theme.font_bold{font_size: 8.0}
            }
        }
        prop_value := Label{
            width: Fill
            draw_text +: {
                color: mod.mpf.fg_bright
                text_style: theme.font_regular{font_size: 9.5}
            }
        }
    }

    // The omarchy popup card: the theme background behind a 2px accent edge,
    // hard corners, 28pt rows, and a hover that is the foreground at 8% with
    // accent text. No fade — a menu that animates in is a menu you wait for.
    let CtxRow = View{
        visible: false
        width: Fill
        height: Fit
        flow: Down
        ctx_line := SolidView{
            visible: false
            width: Fill
            height: 1
            margin: Inset{top: 4 bottom: 4}
            draw_bg +: {color: mod.mpf.muted}
        }
        ctx_body := SolidView{
            width: Fill
            height: 28
            flow: Right
            spacing: 12
            padding: Inset{left: 14 right: 14}
            align: Align{y: 0.5}
            cursor: MouseCursor.Hand
            draw_bg +: {color: mod.mpf.bg}
            ctx_label := Label{
                width: Fill
                max_lines: 1
                text_overflow: TextOverflow.Ellipsis
                draw_text +: {
                    color: mod.mpf.fg
                    text_style: theme.font_regular{font_size: 9.5}
                }
            }
            ctx_hint := Label{
                draw_text +: {
                    color: mod.mpf.fg_dim
                    text_style: theme.font_regular{font_size: 8.5}
                }
            }
        }
    }

    let CtxPanel = RectView{
        width: 268
        height: Fit
        flow: Down
        padding: Inset{top: 5 bottom: 5}
        draw_bg +: {
            color: mod.mpf.bg
            border_color: mod.mpf.accent
            border_size: 2.0
        }
    }

    let DialogButton = RectView{
        width: Fit
        height: 28
        padding: Inset{left: 16 right: 16}
        align: Align{x: 0.5 y: 0.5}
        cursor: MouseCursor.Hand
        draw_bg +: {
            color: mod.mpf.bg_light
            border_color: mod.mpf.muted
            border_size: 1.0
        }
        dlg_label := Label{
            draw_text +: {
                color: mod.mpf.fg_bright
                text_style: theme.font_regular{font_size: 9.5}
            }
        }
    }

    let DialogField = View{
        width: Fill
        height: Fit
        flow: Down
        spacing: 4
        padding: Inset{top: 6 bottom: 6}
        field_key := Label{
            draw_text +: {
                color: mod.mpf.fg_dim
                text_style: theme.font_bold{font_size: 8.0}
            }
        }
        field_box := View{
            width: Fill
            height: 26
            field_input := MpfInput{}
        }
    }

    mod.widgets.BreadcrumbsBase = #(Breadcrumbs::register_widget(vm))
    mod.widgets.Breadcrumbs = set_type_default() do mod.widgets.BreadcrumbsBase{
        width: Fit
        height: Fill
        flow: Right
        spacing: 1
        align: Align{y: 0.5}
        clip_x: true
        c0 := Crumb{}
        s0 := CrumbSep{}
        c1 := Crumb{}
        s1 := CrumbSep{}
        c2 := Crumb{}
        s2 := CrumbSep{}
        c3 := Crumb{}
        s3 := CrumbSep{}
        c4 := Crumb{}
        s4 := CrumbSep{}
        c5 := Crumb{}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Files"
                window.inner_size: vec2(1240, 800)
                pass.clear_color: mod.mpf.bg
                body +: {
                    flow: Overlay
                    app_bg := SolidView{
                        width: Fill
                        height: Fill
                        flow: Down
                        draw_bg +: {color: mod.mpf.bg}

                        top_bar := SolidView{
                            width: Fill
                            height: 38
                            flow: Right
                            spacing: 4
                            padding: Inset{right: 10}
                            align: Align{y: 0.5}
                            draw_bg +: {color: mod.mpf.bg_light}

                            title_box := View{
                                width: 208
                                height: Fill
                                padding: Inset{left: 18}
                                align: Align{y: 0.5}
                                files_title := Label{
                                    text: "Files"
                                    draw_text +: {
                                        color: mod.mpf.fg_bright
                                        text_style: theme.font_bold{font_size: 11.0}
                                    }
                                }
                            }

                            back_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/back.svg")
                                        color: mod.mpf.fg
                                    }
                                }
                            }
                            forward_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/forward.svg")
                                        color: mod.mpf.fg
                                    }
                                }
                            }

                            path_box := RectView{
                                width: Fill
                                height: 28
                                flow: Right
                                margin: Inset{left: 4 right: 4}
                                padding: Inset{left: 5 right: 5}
                                align: Align{y: 0.5}
                                draw_bg +: {
                                    color: mod.mpf.bg
                                    border_color: mod.mpf.muted
                                    border_size: 1.0
                                }
                                // The crumb box fills the plate so a click in
                                // the empty space past the last crumb still
                                // lands somewhere — that is what opens the
                                // editable path.
                                crumb_box := View{
                                    width: Fill
                                    height: Fill
                                    align: Align{y: 0.5}
                                    cursor: MouseCursor.Text
                                    breadcrumbs := mod.widgets.Breadcrumbs{}
                                }
                                path_edit_box := View{
                                    visible: false
                                    width: Fill
                                    height: Fill
                                    path_edit := MpfInput{
                                        empty_text: "Type a path"
                                        draw_bg +: {
                                            border_size: uniform(0.0)
                                        }
                                    }
                                }
                                search_box := View{
                                    visible: false
                                    width: Fill
                                    height: Fill
                                    search_input := MpfInput{
                                        empty_text: "Search this folder"
                                        draw_bg +: {
                                            border_size: uniform(0.0)
                                        }
                                    }
                                }
                            }

                            icons_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/grid.svg")
                                        color: mod.mpf.fg
                                    }
                                }
                            }
                            list_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/list.svg")
                                        color: mod.mpf.fg
                                    }
                                }
                            }
                            compact_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/compact.svg")
                                        color: mod.mpf.fg
                                    }
                                }
                            }
                            treemap_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/treemap.svg")
                                        color: mod.mpf.fg
                                    }
                                }
                            }

                            View{width: 6 height: 1}

                            newfolder_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/newfolder.svg")
                                        color: mod.mpf.fg
                                    }
                                }
                            }
                            terminal_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/terminal.svg")
                                        color: mod.mpf.fg
                                    }
                                }
                            }
                            props_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/info.svg")
                                        color: mod.mpf.fg
                                    }
                                }
                            }
                            search_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/search.svg")
                                        color: mod.mpf.fg
                                    }
                                }
                            }
                            preview_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/eye.svg")
                                        color: mod.mpf.fg
                                    }
                                }
                            }
                            menu_button := ToolButton{
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/menu-dots.svg")
                                        color: mod.mpf.fg
                                    }
                                }
                            }
                        }

                        tab_strip := SolidView{
                            visible: false
                            width: Fill
                            height: 26
                            flow: Right
                            spacing: 1
                            padding: Inset{left: 1}
                            draw_bg +: {color: mod.mpf.bg}
                            tab0 := TabItem{}
                            tab1 := TabItem{}
                            tab2 := TabItem{}
                            tab3 := TabItem{}
                            tab4 := TabItem{}
                            tab5 := TabItem{}
                            tab6 := TabItem{}
                            tab7 := TabItem{}
                        }

                        body_row := View{
                            width: Fill
                            height: Fill
                            flow: Right

                            sidebar := SolidView{
                                width: 208
                                height: Fill
                                flow: Down
                                draw_bg +: {color: mod.mpf.bg_dark}

                                side_scroll := ScrollYView{
                                    width: Fill
                                    height: Fill
                                    flow: Down
                                    padding: Inset{top: 10 bottom: 10}

                                    home_item := SideItem{
                                        side_icon +: {draw_icon +: {svg: crate_resource("self://resources/icons/home.svg")}}
                                        side_title +: {text: "Home"}
                                    }
                                    recent_item := SideItem{
                                        side_icon +: {draw_icon +: {svg: crate_resource("self://resources/icons/clock.svg")}}
                                        side_title +: {text: "Recent"}
                                    }
                                    starred_item := SideItem{
                                        side_icon +: {draw_icon +: {svg: crate_resource("self://resources/icons/star.svg")}}
                                        side_title +: {text: "Starred"}
                                    }
                                    network_item := SideItem{
                                        side_icon +: {draw_icon +: {svg: crate_resource("self://resources/icons/network.svg")}}
                                        side_title +: {text: "Network"}
                                    }
                                    trash_item := SideItem{
                                        side_icon +: {draw_icon +: {svg: crate_resource("self://resources/icons/trash.svg")}}
                                        side_title +: {text: "Trash"}
                                    }

                                    Divider{}
                                    SectionLabel{text: "PLACES"}

                                    desktop_item := SideItem{
                                        side_icon +: {draw_icon +: {svg: crate_resource("self://resources/icons/folder.svg")}}
                                        side_title +: {text: "Desktop"}
                                    }
                                    documents_item := SideItem{
                                        side_icon +: {draw_icon +: {svg: crate_resource("self://resources/icons/folder.svg")}}
                                        side_title +: {text: "Documents"}
                                    }
                                    downloads_item := SideItem{
                                        side_icon +: {draw_icon +: {svg: crate_resource("self://resources/icons/folder.svg")}}
                                        side_title +: {text: "Downloads"}
                                    }
                                    music_item := SideItem{
                                        side_icon +: {draw_icon +: {svg: crate_resource("self://resources/icons/folder.svg")}}
                                        side_title +: {text: "Music"}
                                    }
                                    pictures_item := SideItem{
                                        side_icon +: {draw_icon +: {svg: crate_resource("self://resources/icons/folder.svg")}}
                                        side_title +: {text: "Pictures"}
                                    }
                                    videos_item := SideItem{
                                        side_icon +: {draw_icon +: {svg: crate_resource("self://resources/icons/folder.svg")}}
                                        side_title +: {text: "Videos"}
                                    }

                                    Divider{}
                                    SectionLabel{text: "BOOKMARKS"}
                                    bookmark_hint := Label{
                                        width: Fill
                                        height: 34
                                        padding: Inset{left: 18 right: 12}
                                        max_lines: 2
                                        text: "Cmd+D bookmarks this folder, or drag one here"
                                        draw_text +: {
                                            color: mod.mpf.fg_dim
                                            text_style: theme.font_regular{font_size: 8.0}
                                        }
                                    }
                                    bm0 := BookmarkItem{}
                                    bm1 := BookmarkItem{}
                                    bm2 := BookmarkItem{}
                                    bm3 := BookmarkItem{}
                                    bm4 := BookmarkItem{}
                                    bm5 := BookmarkItem{}
                                    bm6 := BookmarkItem{}
                                    bm7 := BookmarkItem{}
                                    bm8 := BookmarkItem{}
                                    bm9 := BookmarkItem{}
                                    bm10 := BookmarkItem{}
                                    bm11 := BookmarkItem{}
                                }
                                hidden_hint := Label{
                                    width: Fill
                                    height: 22
                                    padding: Inset{left: 18}
                                    text: "Ctrl+H  Show hidden files"
                                    draw_text +: {
                                        color: mod.mpf.fg_dim
                                        text_style: theme.font_regular{font_size: 8.0}
                                    }
                                }
                            }

                            content_bg := SolidView{
                                width: Fill
                                height: Fill
                                flow: Down
                                draw_bg +: {color: mod.mpf.bg}

                                folder_header := View{
                                    width: Fill
                                    height: 44
                                    flow: Right
                                    padding: Inset{left: 20 right: 20}
                                    align: Align{y: 0.5}
                                    folder_title := Label{
                                        width: Fill
                                        max_lines: 1
                                        text_overflow: TextOverflow.Ellipsis
                                        text: "Home"
                                        draw_text +: {
                                            color: mod.mpf.fg_bright
                                            text_style: theme.font_bold{font_size: 12.0}
                                        }
                                    }
                                    item_count := Label{
                                        text: "Loading…"
                                        draw_text +: {
                                            color: mod.mpf.fg_dim
                                            text_style: theme.font_regular{font_size: 9.0}
                                        }
                                    }
                                }
                                empty_label := Label{
                                    visible: false
                                    width: Fill
                                    height: 46
                                    padding: Inset{left: 20}
                                    text: "This folder is empty"
                                    draw_text +: {
                                        color: mod.mpf.fg_dim
                                        text_style: theme.font_regular{font_size: 10.0}
                                    }
                                }
                                contents := mod.widgets.FileContents{}

                                progress_row := SolidView{
                                    visible: false
                                    width: Fill
                                    height: 30
                                    flow: Right
                                    spacing: 12
                                    padding: Inset{left: 16 right: 12}
                                    align: Align{y: 0.5}
                                    draw_bg +: {color: mod.mpf.bg_light}
                                    progress_track := SolidView{
                                        width: 150
                                        height: 6
                                        draw_bg +: {color: mod.mpf.muted}
                                        progress_fill := SolidView{
                                            width: 0
                                            height: Fill
                                            draw_bg +: {color: mod.mpf.accent}
                                        }
                                    }
                                    progress_label := Label{
                                        width: Fill
                                        max_lines: 1
                                        text_overflow: TextOverflow.Ellipsis
                                        draw_text +: {
                                            color: mod.mpf.fg
                                            text_style: theme.font_regular{font_size: 8.5}
                                        }
                                    }
                                    progress_cancel := View{
                                        width: Fit
                                        height: 22
                                        padding: Inset{left: 10 right: 10}
                                        align: Align{y: 0.5}
                                        cursor: MouseCursor.Hand
                                        Label{
                                            text: "Cancel"
                                            draw_text +: {
                                                color: mod.mpf.accent
                                                text_style: theme.font_regular{font_size: 8.5}
                                            }
                                        }
                                    }
                                }

                                status_bar := SolidView{
                                    width: Fill
                                    height: 26
                                    padding: Inset{left: 16 right: 16}
                                    align: Align{y: 0.5}
                                    draw_bg +: {color: mod.mpf.bg_dark}
                                    status_label := Label{
                                        width: Fill
                                        max_lines: 1
                                        text_overflow: TextOverflow.Ellipsis
                                        text: "Loading…"
                                        draw_text +: {
                                            color: mod.mpf.fg_dim
                                            text_style: theme.font_regular{font_size: 8.5}
                                        }
                                    }
                                }
                            }

                            props_panel := SolidView{
                                visible: false
                                width: 306
                                height: Fill
                                flow: Down
                                draw_bg +: {color: mod.mpf.bg_dark}
                                props_header := SolidView{
                                    width: Fill
                                    height: 36
                                    flow: Right
                                    padding: Inset{left: 16 right: 10}
                                    align: Align{y: 0.5}
                                    draw_bg +: {color: mod.mpf.bg_light}
                                    Label{
                                        width: Fill
                                        text: "Properties"
                                        draw_text +: {
                                            color: mod.mpf.fg_bright
                                            text_style: theme.font_bold{font_size: 10.0}
                                        }
                                    }
                                    props_close := View{
                                        width: 20
                                        height: 20
                                        align: Align{x: 0.5 y: 0.5}
                                        cursor: MouseCursor.Hand
                                        Icon{
                                            icon_walk: Walk{width: 10 height: 10}
                                            draw_icon +: {
                                                svg: crate_resource("self://resources/icons/close.svg")
                                                color: mod.mpf.fg_dim
                                            }
                                        }
                                    }
                                }
                                props_scroll := ScrollYView{
                                    width: Fill
                                    height: Fill
                                    flow: Down
                                    prop_name := PropRow{prop_key +: {text: "NAME"}}
                                    prop_kind := PropRow{prop_key +: {text: "KIND"}}
                                    prop_size := PropRow{
                                        prop_key +: {text: "SIZE"}
                                        prop_spinner := LoadingSpinner{
                                            visible: false
                                            width: 18
                                            height: 18
                                        }
                                    }
                                    prop_modified := PropRow{prop_key +: {text: "MODIFIED"}}
                                    prop_created := PropRow{prop_key +: {text: "CREATED"}}
                                    prop_permissions := PropRow{prop_key +: {text: "PERMISSIONS"}}
                                    prop_path := PropRow{prop_key +: {text: "WHERE"}}
                                    prop_opens := PropRow{prop_key +: {text: "OPEN WITH"}}
                                }
                            }
                        }
                    }

                    column_menu := View{
                        visible: false
                        width: Fill
                        height: Fill
                        align: Align{x: 1.0 y: 0.0}
                        padding: Inset{top: 50 right: 10}
                        menu_panel := RectView{
                            width: 210
                            height: Fit
                            flow: Down
                            padding: Inset{top: 6 bottom: 6}
                            draw_bg +: {
                                color: mod.mpf.bg_dark
                                border_color: mod.mpf.muted
                                border_size: 1.0
                            }
                            menu_title := Label{
                                height: 26
                                padding: Inset{left: 14}
                                text: "COLUMNS"
                                draw_text +: {
                                    color: mod.mpf.fg_dim
                                    text_style: theme.font_bold{font_size: 8.0}
                                }
                            }
                            menu_size := MenuRow{}
                            menu_kind := MenuRow{}
                            menu_modified := MenuRow{}
                            menu_created := MenuRow{}
                            menu_permissions := MenuRow{}
                        }
                    }

                    context_menu := View{
                        visible: false
                        width: Fill
                        height: Fill
                        align: Align{x: 0.0 y: 0.0}
                        padding: Inset{left: 0 top: 0}
                        ctx_panel := CtxPanel{
                            ctx0 := CtxRow{}
                            ctx1 := CtxRow{}
                            ctx2 := CtxRow{}
                            ctx3 := CtxRow{}
                            ctx4 := CtxRow{}
                            ctx5 := CtxRow{}
                            ctx6 := CtxRow{}
                            ctx7 := CtxRow{}
                            ctx8 := CtxRow{}
                            ctx9 := CtxRow{}
                            ctx10 := CtxRow{}
                            ctx11 := CtxRow{}
                            ctx12 := CtxRow{}
                            ctx13 := CtxRow{}
                        }
                    }

                    context_submenu := View{
                        visible: false
                        width: Fill
                        height: Fill
                        align: Align{x: 0.0 y: 0.0}
                        padding: Inset{left: 0 top: 0}
                        ctx_sub_panel := CtxPanel{
                            sub0 := CtxRow{}
                            sub1 := CtxRow{}
                            sub2 := CtxRow{}
                            sub3 := CtxRow{}
                        }
                    }

                    batch_dialog := SolidView{
                        visible: false
                        width: Fill
                        height: Fill
                        align: Align{x: 0.5 y: 0.5}
                        draw_bg +: {color: #x0b0b10cc}
                        batch_panel := RectView{
                            width: 540
                            height: Fit
                            flow: Down
                            padding: Inset{left: 20 right: 20 top: 16 bottom: 16}
                            draw_bg +: {
                                color: mod.mpf.bg_dark
                                border_color: mod.mpf.muted
                                border_size: 1.0
                            }
                            batch_title := Label{
                                text: "Rename 0 files"
                                draw_text +: {
                                    color: mod.mpf.fg_bright
                                    text_style: theme.font_bold{font_size: 11.0}
                                }
                            }
                            batch_find := DialogField{
                                field_key +: {text: "FIND"}
                                field_box +: {field_input +: {empty_text: "text in the current names"}}
                            }
                            batch_replace := DialogField{
                                field_key +: {text: "REPLACE WITH"}
                                field_box +: {field_input +: {empty_text: "what to put there instead"}}
                            }
                            batch_pattern := DialogField{
                                field_key +: {text: "OR A PATTERN — {name} IS THE OLD NAME, ### THE NUMBER"}
                                field_box +: {field_input +: {empty_text: "shot-###"}}
                            }
                            batch_preview := Label{
                                width: Fill
                                height: Fit
                                margin: Inset{top: 10 bottom: 10}
                                draw_text +: {
                                    color: mod.mpf.fg_dim
                                    text_style: theme.font_code{font_size: 8.5}
                                }
                            }
                            batch_buttons := View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 10
                                align: Align{x: 1.0}
                                batch_cancel := DialogButton{dlg_label +: {text: "Cancel"}}
                                batch_apply := DialogButton{
                                    dlg_label +: {text: "Rename"}
                                    draw_bg +: {
                                        color: mod.mpf.sel
                                        border_color: mod.mpf.accent
                                    }
                                }
                            }
                        }
                    }

                    quick_look := SolidView{
                        visible: false
                        width: Fill
                        height: Fill
                        align: Align{x: 0.5 y: 0.5}
                        draw_bg +: {color: #x0b0b10cc}
                        ql_panel := RectView{
                            width: 780
                            height: 560
                            flow: Down
                            draw_bg +: {
                                color: mod.mpf.bg_dark
                                border_color: mod.mpf.muted
                                border_size: 1.0
                            }
                            ql_header := SolidView{
                                width: Fill
                                height: 36
                                flow: Right
                                padding: Inset{left: 14 right: 14}
                                align: Align{y: 0.5}
                                draw_bg +: {color: mod.mpf.bg_light}
                                ql_title := Label{
                                    width: Fill
                                    max_lines: 1
                                    text_overflow: TextOverflow.Ellipsis
                                    text: "Preview"
                                    draw_text +: {
                                        color: mod.mpf.fg_bright
                                        text_style: theme.font_bold{font_size: 10.0}
                                    }
                                }
                                ql_hint := Label{
                                    text: "Space or Esc to close"
                                    draw_text +: {
                                        color: mod.mpf.fg_dim
                                        text_style: theme.font_regular{font_size: 8.5}
                                    }
                                }
                            }
                            ql_scroll := ScrollYView{
                                width: Fill
                                height: Fill
                                padding: Inset{left: 14 right: 14 top: 10 bottom: 10}
                                ql_text := Label{
                                    width: Fill
                                    draw_text +: {
                                        color: mod.mpf.fg
                                        text_style: theme.font_code{font_size: 8.5}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The 6 crumb slots of the path bar; deeper paths show their tail.
const CRUMB_IDS: [&[LiveId]; 6] = [
    ids!(c0),
    ids!(c1),
    ids!(c2),
    ids!(c3),
    ids!(c4),
    ids!(c5),
];
const CRUMB_TITLE_IDS: [&[LiveId]; 6] = [
    ids!(c0.crumb_title),
    ids!(c1.crumb_title),
    ids!(c2.crumb_title),
    ids!(c3.crumb_title),
    ids!(c4.crumb_title),
    ids!(c5.crumb_title),
];
const CRUMB_SEP_IDS: [&[LiveId]; 5] =
    [ids!(s0), ids!(s1), ids!(s2), ids!(s3), ids!(s4)];

/// The path bar: the tail of the current path, each part clickable.
#[derive(Script, ScriptHook, Widget)]
pub struct Breadcrumbs {
    #[deref]
    view: View,
    #[rust]
    paths: Vec<PathBuf>,
}

impl Breadcrumbs {
    fn set_path(&mut self, cx: &mut Cx, path: &Path) {
        let mut paths: Vec<PathBuf> = path.ancestors().map(Path::to_path_buf).collect();
        paths.reverse();
        if paths.len() > CRUMB_IDS.len() {
            paths = paths.split_off(paths.len() - CRUMB_IDS.len());
        }
        self.paths = paths;
        for (index, crumb) in CRUMB_IDS.iter().enumerate() {
            let visible = index < self.paths.len();
            self.view.view(cx, *crumb).set_visible(cx, visible);
            if visible {
                let title = display_name(&self.paths[index]);
                self.view
                    .label(cx, CRUMB_TITLE_IDS[index])
                    .set_text(cx, &title);
            }
            if let Some(sep) = CRUMB_SEP_IDS.get(index) {
                self.view
                    .widget(cx, *sep)
                    .set_visible(cx, index + 1 < self.paths.len());
            }
        }
        self.view.redraw(cx);
    }

    fn clicked_path(&self, cx: &mut Cx, actions: &Actions) -> Option<PathBuf> {
        for (index, crumb) in CRUMB_IDS.iter().enumerate() {
            if self.view.view(cx, *crumb).finger_down(actions).is_some() {
                return self.paths.get(index).cloned();
            }
        }
        None
    }
}

impl Widget for Breadcrumbs {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

/// One browser tab: its folder, its own history, and its own view mode. GNOME
/// Files keeps all three per tab, and anything less makes tabs a lie —
/// switching back would land you somewhere you never were.
#[derive(Clone, Debug)]
pub struct Tab {
    dir: PathBuf,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    mode: ViewMode,
}

impl Tab {
    fn new(dir: PathBuf, mode: ViewMode) -> Self {
        Self {
            dir,
            back: Vec::new(),
            forward: Vec::new(),
            mode,
        }
    }
}

/// A finished directory read, matched back to the request that asked for it.
struct DirectoryResult {
    /// The tab folder the read belongs to; a listing for a folder we already
    /// left answers no question anybody is still asking.
    dir: PathBuf,
    request_id: u64,
    /// `None` for the folder itself, `Some` for the children of a folder the
    /// List tree expanded.
    parent: Option<PathBuf>,
    result: Result<Vec<FileEntry>, String>,
}

/// Which field should take the keyboard once it has been drawn. A widget that
/// was hidden a moment ago has no area yet, and `take_key_focus` on an empty
/// area focuses nothing — so every reveal-then-focus goes through here and
/// lands one frame later.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum FocusTarget {
    #[default]
    None,
    Path,
    Search,
    Batch,
}

/// A finished recursive size measurement for the properties panel.
struct SizeResult {
    path: PathBuf,
    bytes: u64,
}

/// The sidebar places, in order: (widget id, what it navigates to).
const PLACES: [(&[LiveId], &str); 11] = [
    (ids!(home_item), "home"),
    (ids!(recent_item), "recent"),
    (ids!(starred_item), "starred"),
    (ids!(network_item), "network"),
    (ids!(trash_item), "trash"),
    (ids!(desktop_item), "Desktop"),
    (ids!(documents_item), "Documents"),
    (ids!(downloads_item), "Downloads"),
    (ids!(music_item), "Music"),
    (ids!(pictures_item), "Pictures"),
    (ids!(videos_item), "Videos"),
];

/// The column picker's rows: (row id, the column it toggles).
/// Name is the row's identity and is not offered.
const COLUMN_ROWS: [(&[LiveId], model::SortKey); 5] = [
    (ids!(menu_size), model::SortKey::Size),
    (ids!(menu_kind), model::SortKey::Kind),
    (ids!(menu_modified), model::SortKey::Modified),
    (ids!(menu_created), model::SortKey::Created),
    (ids!(menu_permissions), model::SortKey::Permissions),
];

const MODE_BUTTONS: [(&[LiveId], ViewMode); 4] = [
    (ids!(icons_button), ViewMode::Icons),
    (ids!(list_button), ViewMode::List),
    (ids!(compact_button), ViewMode::Compact),
    (ids!(treemap_button), ViewMode::Treemap),
];

/// The tab strip's slots. More tabs than this and the strip would be a
/// horizontal scroll problem instead of a tab strip.
const TAB_IDS: [&[LiveId]; 8] = [
    ids!(tab0),
    ids!(tab1),
    ids!(tab2),
    ids!(tab3),
    ids!(tab4),
    ids!(tab5),
    ids!(tab6),
    ids!(tab7),
];

/// The sidebar's bookmark slots — as many as [`bookmarks::MAX_BOOKMARKS`].
const BOOKMARK_IDS: [&[LiveId]; bookmarks::MAX_BOOKMARKS] = [
    ids!(bm0),
    ids!(bm1),
    ids!(bm2),
    ids!(bm3),
    ids!(bm4),
    ids!(bm5),
    ids!(bm6),
    ids!(bm7),
    ids!(bm8),
    ids!(bm9),
    ids!(bm10),
    ids!(bm11),
];

/// The context menu's row slots — as many as [`menu::MAX_ROWS`].
const CTX_IDS: [&[LiveId]; menu::MAX_ROWS] = [
    ids!(ctx0),
    ids!(ctx1),
    ids!(ctx2),
    ids!(ctx3),
    ids!(ctx4),
    ids!(ctx5),
    ids!(ctx6),
    ids!(ctx7),
    ids!(ctx8),
    ids!(ctx9),
    ids!(ctx10),
    ids!(ctx11),
    ids!(ctx12),
    ids!(ctx13),
];

/// The Open With submenu's slots.
const CTX_SUB_IDS: [&[LiveId]; menu::MAX_APPS] =
    [ids!(sub0), ids!(sub1), ids!(sub2), ids!(sub3)];

/// One menu row's height, and the space a separator adds above it — the two
/// numbers the panel's own height is made of, so it can be kept on screen.
const CTX_ROW_H: f64 = 28.0;
const CTX_SEP_H: f64 = 9.0;
const CTX_PANEL_W: f64 = 268.0;

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    tabs: Vec<Tab>,
    #[rust]
    tab: usize,
    #[rust]
    home: PathBuf,
    #[rust]
    sender: Option<Sender<DirectoryResult>>,
    #[rust]
    receiver: Option<Receiver<DirectoryResult>>,
    #[rust]
    size_sender: Option<Sender<SizeResult>>,
    #[rust]
    size_receiver: Option<Receiver<SizeResult>>,
    #[rust]
    size_cancel: Option<Arc<AtomicBool>>,
    #[rust]
    request_id: u64,
    #[rust]
    show_hidden: bool,
    #[rust]
    search_visible: bool,
    #[rust]
    path_edit_open: bool,
    #[rust]
    preview: PreviewHost,
    #[rust]
    quick_look_open: bool,
    #[rust]
    column_menu_open: bool,
    #[rust]
    props_open: bool,
    #[rust]
    batch_open: bool,
    #[rust]
    bookmarks: Bookmarks,
    #[rust]
    hovered_bookmark: Option<usize>,
    #[rust]
    focus_next: NextFrame,
    #[rust]
    focus_target: FocusTarget,
    #[rust]
    focus_tries: usize,
    /// Copy and cut share one clipboard; `clipboard_cut` says which it was.
    #[rust]
    clipboard: Vec<PathBuf>,
    #[rust]
    clipboard_cut: bool,
    #[rust]
    ops: Option<Ops>,
    #[rust]
    journal: Journal,
    #[rust]
    op_id: u64,
    /// The job the progress row is showing, so Cancel knows what to stop.
    #[rust]
    active_op: Option<u64>,
    /// A file to select, and maybe rename, once the folder is re-listed.
    #[rust]
    pending_select: Vec<PathBuf>,
    #[rust]
    pending_rename: Option<PathBuf>,
    /// What an operation said it did. The re-listing that follows would
    /// overwrite the status line with the folder's resting state, and a
    /// cancelled copy or an undo the user never sees is one they cannot trust.
    #[rust]
    pending_status: Option<String>,
    /// The paths the batch dialog is about to rename.
    #[rust]
    batch_targets: Vec<PathBuf>,
    /// The context menu: what it offers, what it is about, and which row the
    /// pointer is on.
    #[rust]
    menu_open: bool,
    #[rust]
    menu_rows: Vec<MenuRow>,
    #[rust]
    menu_target: Option<FileEntry>,
    #[rust]
    menu_hover: Option<usize>,
    #[rust]
    submenu_open: bool,
    #[rust]
    submenu_apps: Vec<AppChoice>,
    #[rust]
    submenu_hover: Option<usize>,
    /// A permanent delete that has been asked for once and is waiting for the
    /// second press that means it. There is no undo behind this one, so it is
    /// the only thing in the app that asks twice.
    #[rust]
    pending_delete: Vec<PathBuf>,
}

/// One row of the Open With submenu: the app's id (empty = the desktop's own
/// opener) and the sentence the row shows.
#[derive(Clone, Debug, Default)]
pub struct AppChoice {
    id: String,
    label: String,
}

impl App {
    // ----------------------------------------------------------------- tabs

    fn current_dir(&self) -> PathBuf {
        self.tabs
            .get(self.tab)
            .map(|t| t.dir.clone())
            .unwrap_or_default()
    }

    fn tab_mut(&mut self) -> &mut Tab {
        let index = self.tab.min(self.tabs.len().saturating_sub(1));
        &mut self.tabs[index]
    }

    fn new_tab(&mut self, cx: &mut Cx) {
        if self.tabs.len() >= TAB_IDS.len() {
            self.status(cx, "That is as many tabs as the strip holds");
            return;
        }
        let dir = self.current_dir();
        let mode = self.tabs[self.tab].mode;
        self.tabs.insert(self.tab + 1, Tab::new(dir, mode));
        self.tab += 1;
        self.enter_tab(cx);
    }

    /// Close the active tab. The last tab has nothing behind it, so closing
    /// it closes the window — which is what Cmd+W means everywhere else.
    fn close_tab(&mut self, cx: &mut Cx) {
        if self.tabs.len() <= 1 {
            cx.quit();
            return;
        }
        self.tabs.remove(self.tab);
        self.tab = self.tab.min(self.tabs.len() - 1);
        self.enter_tab(cx);
    }

    fn switch_tab(&mut self, cx: &mut Cx, delta: isize) {
        if self.tabs.len() < 2 {
            return;
        }
        let count = self.tabs.len() as isize;
        self.tab = ((self.tab as isize + delta).rem_euclid(count)) as usize;
        self.enter_tab(cx);
    }

    /// Make the active tab's state the window's state.
    fn enter_tab(&mut self, cx: &mut Cx) {
        let mode = self.tabs[self.tab].mode;
        self.apply_mode(cx, mode);
        self.refresh_tab_strip(cx);
        self.request_directory(cx);
    }

    fn refresh_tab_strip(&mut self, cx: &mut Cx) {
        // One tab is not a tab strip: it is a window, and a strip over it is
        // just a bar that says nothing.
        let many = self.tabs.len() > 1;
        self.ui.widget(cx, ids!(tab_strip)).set_visible(cx, many);
        let palette = Palette::shared();
        let (on, off) = (
            Palette::vec4(&palette.bg_light),
            Palette::vec4(&palette.bg_dark),
        );
        for (index, id) in TAB_IDS.iter().enumerate() {
            let shown = many && index < self.tabs.len();
            let mut item = self.ui.view(cx, *id);
            item.set_visible(cx, shown);
            if !shown {
                continue;
            }
            let title = display_name(&self.tabs[index].dir);
            item.label(cx, ids!(tab_title)).set_text(cx, &title);
            let color = if index == self.tab { on } else { off };
            script_apply_eval!(cx, item, {
                draw_bg +: {color: #(color)}
            });
        }
    }

    // --------------------------------------------------------------- status

    /// Give the keyboard to `target` on the next frame, once it has been drawn
    /// and has an area to focus.
    fn focus_soon(&mut self, cx: &mut Cx, target: FocusTarget) {
        self.focus_target = target;
        self.focus_tries = 0;
        self.focus_next = cx.new_next_frame();
    }

    fn apply_focus(&mut self, cx: &mut Cx) {
        let field = match self.focus_target {
            FocusTarget::None => return,
            FocusTarget::Path => self.ui.text_input(cx, ids!(path_edit)),
            FocusTarget::Search => self.ui.text_input(cx, ids!(search_input)),
            FocusTarget::Batch => self
                .ui
                .view(cx, ids!(batch_find))
                .text_input(cx, ids!(field_input)),
        };
        // `take_key_focus` focuses the field's *area*, and a field that has
        // not been drawn since it was revealed has none — focusing it would
        // quietly focus nothing. Wait for the frame that gives it one.
        let drawn = field.area().rect(cx).size.x >= 1.0;
        if !drawn {
            if self.focus_tries < 8 {
                self.focus_tries += 1;
                self.focus_next = cx.new_next_frame();
            }
            drop(field);
            return;
        }
        self.focus_target = FocusTarget::None;
        self.focus_tries = 0;
        field.take_key_focus(cx);
        {
            // The borrow has to end before `field` does.
            if let Some(mut inner) = field.borrow_mut() {
                inner.select_all(cx);
            }
        }
        drop(field);
    }

    fn status(&mut self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, text);
    }

    /// True when a press at `at` landed on `child` *inside this row*.
    ///
    /// A clickable row that contains a clickable button is one press, not two:
    /// the row is what reports it, and the button inside never sees a
    /// `FingerDown` of its own. So which of the two was meant is decided by
    /// geometry — where the finger actually went down. The lookup has to start
    /// at the row, because every row in a list carries the same child id and a
    /// search from the root would always answer with the first one.
    fn pressed_on(&mut self, cx: &mut Cx, row: &[LiveId], child: &[LiveId], at: DVec2) -> bool {
        let rect = self
            .ui
            .view(cx, row)
            .widget(cx, child)
            .area()
            .rect(cx);
        rect.size.x > 0.0 && rect.contains(at)
    }

    fn with_contents<R>(
        &mut self,
        cx: &mut Cx,
        f: impl FnOnce(&mut FileContents, &mut Cx) -> R,
    ) -> Option<R> {
        let widget = self.ui.widget(cx, ids!(contents));
        let mut contents = widget.borrow_mut::<FileContents>()?;
        Some(f(&mut contents, cx))
    }

    // ---------------------------------------------------------- navigation

    fn navigate(&mut self, cx: &mut Cx, path: PathBuf, add_history: bool) {
        let current = self.current_dir();
        if add_history && !current.as_os_str().is_empty() && current != path {
            let tab = self.tab_mut();
            tab.back.push(current);
            tab.forward.clear();
        }
        self.tab_mut().dir = path;
        self.request_directory(cx);
    }

    fn request_directory(&mut self, cx: &mut Cx) {
        let Some(sender) = self.sender.clone() else {
            return;
        };
        self.request_id = self.request_id.wrapping_add(1);
        let request_id = self.request_id;
        let path = self.current_dir();
        let show_hidden = self.show_hidden;
        self.update_path_ui(cx);
        self.refresh_tab_strip(cx);
        self.ui.label(cx, ids!(item_count)).set_text(cx, "Loading…");
        let display = path.display().to_string();
        self.status(cx, &format!("Loading {}…", display));
        // The treemap is of a folder, so a new folder means a new map.
        if self.tabs[self.tab].mode == ViewMode::Treemap {
            let map = self.with_contents(cx, |contents, cx| contents.treemap(cx));
            if let Some(map) = map {
                map.set_root(cx, &path);
            }
        }
        let dir = path.clone();
        thread::spawn(move || {
            let result = vfs().read_dir(&path, show_hidden);
            let sent = sender.send(DirectoryResult {
                dir,
                request_id,
                parent: None,
                result,
            });
            if sent.is_ok() {
                SignalToUI::set_ui_signal();
            }
        });
    }

    /// Read the children of a folder the List tree just expanded.
    fn request_children(&mut self, cx: &mut Cx, folder: PathBuf) {
        let Some(sender) = self.sender.clone() else {
            return;
        };
        let show_hidden = self.show_hidden;
        let dir = self.current_dir();
        let request_id = self.request_id;
        let _ = cx;
        thread::spawn(move || {
            let result = vfs().read_dir(&folder, show_hidden);
            let sent = sender.send(DirectoryResult {
                dir,
                request_id,
                parent: Some(folder),
                result,
            });
            if sent.is_ok() {
                SignalToUI::set_ui_signal();
            }
        });
    }

    fn drain_directory_results(&mut self, cx: &mut Cx) {
        let results: Vec<DirectoryResult> = self
            .receiver
            .as_ref()
            .map(|receiver| receiver.try_iter().collect())
            .unwrap_or_default();
        let current = self.current_dir();
        for result in results {
            if result.dir != current {
                continue;
            }
            if let Some(parent) = result.parent {
                // Children of an expanded folder: the tree keeps its shape
                // even when the read failed, it just opens onto nothing.
                let entries = result.result.unwrap_or_default();
                self.with_contents(cx, |contents, cx| {
                    contents.set_children(cx, &parent, entries)
                });
                continue;
            }
            // A folder the user already left is not the answer to any question.
            if result.request_id != self.request_id {
                continue;
            }
            match result.result {
                Ok(entries) => {
                    let count = entries.len();
                    self.with_contents(cx, |contents, cx| contents.set_entries(cx, entries));
                    self.ui
                        .widget(cx, ids!(empty_label))
                        .set_visible(cx, count == 0);
                    self.ui
                        .label(cx, ids!(empty_label))
                        .set_text(cx, "This folder is empty");
                    self.ui.label(cx, ids!(item_count)).set_text(
                        cx,
                        &format!("{} item{}", count, if count == 1 { "" } else { "s" }),
                    );
                    self.report(cx);
                    self.apply_pending(cx);
                }
                Err(error) => {
                    self.with_contents(cx, |contents, cx| contents.set_entries(cx, Vec::new()));
                    self.ui.widget(cx, ids!(empty_label)).set_visible(cx, true);
                    self.ui
                        .label(cx, ids!(empty_label))
                        .set_text(cx, "Folder unavailable");
                    self.ui.label(cx, ids!(item_count)).set_text(cx, "0 items");
                    self.status(cx, &error);
                }
            }
        }
    }

    /// Select (and maybe start renaming) what an operation just created, now
    /// that the folder has been re-listed and the file is on screen.
    fn apply_pending(&mut self, cx: &mut Cx) {
        let select = std::mem::take(&mut self.pending_select);
        if !select.is_empty() {
            self.with_contents(cx, |contents, cx| contents.select_paths(cx, &select));
        }
        if let Some(path) = self.pending_rename.take() {
            self.with_contents(cx, |contents, cx| contents.begin_rename(cx, &path));
        }
        if let Some(message) = self.pending_status.take() {
            self.status(cx, &message);
        }
    }

    /// The status line's resting state: where we are and how it is sorted.
    fn report(&mut self, cx: &mut Cx) {
        let mode = self.tabs[self.tab].mode;
        if mode == ViewMode::Treemap {
            let text = self
                .with_contents(cx, |contents, cx| contents.treemap(cx).status())
                .unwrap_or_default();
            self.status(cx, &text);
            return;
        }
        let dir = self.current_dir().display().to_string();
        let (sort, picked) = self
            .with_contents(cx, |contents, _| (contents.sort(), contents.selection_count()))
            .unwrap_or_default();
        let key = sort.key.label().to_lowercase();
        let selection = match picked {
            0 => String::new(),
            1 => " · 1 selected".to_string(),
            n => format!(" · {n} selected"),
        };
        let text = format!(
            "{dir} — {} · sorted by {key} {}, folders first{selection}",
            mode.label(),
            if sort.ascending { "↑" } else { "↓" }
        );
        self.status(cx, &text);
    }

    fn update_path_ui(&mut self, cx: &mut Cx) {
        let current = self.current_dir();
        let title = display_name(&current);
        self.ui.label(cx, ids!(folder_title)).set_text(cx, &title);
        let widget = self.ui.widget(cx, ids!(breadcrumbs));
        if let Some(mut breadcrumbs) = widget.borrow_mut::<Breadcrumbs>() {
            breadcrumbs.set_path(cx, &current);
        }
        // Light up the place the current folder belongs to. Recent and
        // Starred are Home shortcuts, not places of their own, so they never
        // claim the highlight.
        let palette = Palette::shared();
        let (on, off) = (Palette::vec4(&palette.sel), Palette::vec4(&palette.bg_dark));
        for (id, name) in PLACES {
            let lit = !matches!(name, "recent" | "starred") && self.place_path(name) == current;
            let color = if lit { on } else { off };
            let mut item = self.ui.view(cx, id);
            script_apply_eval!(cx, item, {
                draw_bg +: {color: #(color)}
            });
        }
        self.refresh_bookmarks(cx);
    }

    fn place_path(&self, name: &str) -> PathBuf {
        match name {
            "home" | "recent" | "starred" => self.home.clone(),
            "network" => PathBuf::from("/"),
            "trash" => trash_dir(&self.home),
            folder => self.home.join(folder),
        }
    }

    fn go_back(&mut self, cx: &mut Cx) {
        let current = self.current_dir();
        let tab = self.tab_mut();
        if let Some(path) = tab.back.pop() {
            tab.forward.push(current);
            tab.dir = path;
            self.request_directory(cx);
        }
    }

    fn go_forward(&mut self, cx: &mut Cx) {
        let current = self.current_dir();
        let tab = self.tab_mut();
        if let Some(path) = tab.forward.pop() {
            tab.back.push(current);
            tab.dir = path;
            self.request_directory(cx);
        }
    }

    fn go_up(&mut self, cx: &mut Cx) {
        if let Some(parent) = self.current_dir().parent() {
            self.navigate(cx, parent.to_path_buf(), true);
        }
    }

    // ---------------------------------------------------------- view modes

    fn set_mode(&mut self, cx: &mut Cx, mode: ViewMode) {
        self.tab_mut().mode = mode;
        self.apply_mode(cx, mode);
        self.report(cx);
        self.ui.redraw(cx);
    }

    /// Push a mode into the body and the toolbar without touching history.
    fn apply_mode(&mut self, cx: &mut Cx, mode: ViewMode) {
        let dir = self.current_dir();
        self.with_contents(cx, |contents, cx| {
            contents.set_mode(cx, mode);
            let map = contents.treemap(cx);
            // Scanning a tree is expensive: it only runs while the map is the
            // thing on screen.
            if mode == ViewMode::Treemap {
                if map.root() != dir {
                    map.set_root(cx, &dir);
                }
            } else {
                map.stop(cx);
            }
        });
        for (id, button_mode) in MODE_BUTTONS {
            self.ui
                .widget(cx, id)
                .widget(cx, ids!(btn_sel))
                .set_visible(cx, button_mode == mode);
        }
    }

    fn zoom(&mut self, cx: &mut Cx, delta: isize) {
        if self.tabs[self.tab].mode != ViewMode::Icons {
            self.status(cx, "Icon sizes are for the Icons view — Cmd+1 switches to it");
            return;
        }
        let level = self
            .with_contents(cx, |contents, _| contents.zoom())
            .unwrap_or(DEFAULT_ZOOM) as isize;
        let next = (level + delta).clamp(0, ZOOM_LEVELS.len() as isize - 1) as usize;
        let width = self
            .with_contents(cx, |contents, cx| contents.set_zoom(cx, next))
            .unwrap_or_default();
        self.status(
            cx,
            &format!(
                "Icon size {} of {} — {:.0}pt tiles",
                next + 1,
                ZOOM_LEVELS.len(),
                width
            ),
        );
    }

    // -------------------------------------------------------------- search

    fn set_search(&mut self, cx: &mut Cx, visible: bool) {
        if visible {
            self.set_path_edit(cx, false);
        }
        self.search_visible = visible;
        self.ui
            .widget(cx, ids!(crumb_box))
            .set_visible(cx, !visible && !self.path_edit_open);
        self.ui.widget(cx, ids!(search_box)).set_visible(cx, visible);
        self.ui
            .widget(cx, ids!(search_button))
            .widget(cx, ids!(btn_sel))
            .set_visible(cx, visible);
        if visible {
            self.focus_soon(cx, FocusTarget::Search);
        } else {
            self.ui.text_input(cx, ids!(search_input)).set_text(cx, "");
            self.with_contents(cx, |contents, cx| contents.set_filter(cx, String::new()));
            cx.set_key_focus(Area::Empty);
        }
        self.ui.redraw(cx);
    }

    // ----------------------------------------------------------- path bar

    /// Ctrl+L, or a click in the empty part of the path plate: the crumbs
    /// become the path, editable.
    fn set_path_edit(&mut self, cx: &mut Cx, open: bool) {
        if open && self.search_visible {
            self.set_search(cx, false);
        }
        self.path_edit_open = open;
        self.ui
            .widget(cx, ids!(crumb_box))
            .set_visible(cx, !open && !self.search_visible);
        self.ui.widget(cx, ids!(path_edit_box)).set_visible(cx, open);
        let field = self.ui.text_input(cx, ids!(path_edit));
        if open {
            let text = self.current_dir().display().to_string();
            field.set_text(cx, &text);
            self.focus_soon(cx, FocusTarget::Path);
            self.status(cx, "Type a path and press Enter — Esc puts the crumbs back");
        } else {
            cx.set_key_focus(Area::Empty);
            self.report(cx);
        }
        self.ui.redraw(cx);
    }

    /// `~` and `~/x` mean the home directory, the way every shell and every
    /// file manager's path box does.
    fn expand_path(&self, text: &str) -> PathBuf {
        let text = text.trim();
        if text == "~" {
            return self.home.clone();
        }
        if let Some(tail) = text.strip_prefix("~/") {
            return self.home.join(tail);
        }
        PathBuf::from(text)
    }

    fn commit_path_edit(&mut self, cx: &mut Cx, text: &str) {
        let path = self.expand_path(text);
        if vfs().is_dir(&path) {
            self.set_path_edit(cx, false);
            self.navigate(cx, path, true);
            return;
        }
        // A file in the box is a reasonable thing to type: open it, and stay
        // where we are.
        if vfs().exists(&path) {
            self.set_path_edit(cx, false);
            let message = preview::open_file(cx, &path);
            self.status(cx, &message);
            return;
        }
        self.status(cx, &format!("{} does not exist", path.display()));
    }

    // ------------------------------------------------------------ bookmarks

    fn refresh_bookmarks(&mut self, cx: &mut Cx) {
        let list: Vec<PathBuf> = self.bookmarks.list().to_vec();
        let current = self.current_dir();
        self.ui
            .widget(cx, ids!(bookmark_hint))
            .set_visible(cx, list.is_empty());
        let palette = Palette::shared();
        let (on, off) = (Palette::vec4(&palette.sel), Palette::vec4(&palette.bg_dark));
        for (index, id) in BOOKMARK_IDS.iter().enumerate() {
            let mut item = self.ui.view(cx, *id);
            let Some(path) = list.get(index) else {
                item.set_visible(cx, false);
                continue;
            };
            item.set_visible(cx, true);
            let title = display_name(path);
            item.label(cx, ids!(bm_title)).set_text(cx, &title);
            item.widget(cx, ids!(bm_remove))
                .set_visible(cx, self.hovered_bookmark == Some(index));
            let color = if *path == current { on } else { off };
            script_apply_eval!(cx, item, {
                draw_bg +: {color: #(color)}
            });
        }
    }

    fn bookmark_current(&mut self, cx: &mut Cx) {
        let path = self.current_dir();
        self.bookmark(cx, path);
    }

    fn bookmark(&mut self, cx: &mut Cx, path: PathBuf) {
        if !vfs().is_dir(&path) {
            self.status(cx, "Only folders can be bookmarked");
            return;
        }
        let name = display_name(&path);
        let message = if self.bookmarks.add(&path) {
            format!("Bookmarked {name}")
        } else if self.bookmarks.contains(&path) {
            format!("{name} is already bookmarked")
        } else {
            "The sidebar has no room for another bookmark".to_string()
        };
        self.refresh_bookmarks(cx);
        self.status(cx, &message);
    }

    // ------------------------------------------------------------- preview

    /// Space: show the selection, or put away what Space last showed.
    fn toggle_preview(&mut self, cx: &mut Cx) {
        if self.quick_look_open {
            self.close_preview(cx);
            return;
        }
        // Whether a panel is open is never this app's own belief. Hosted, the
        // WM's last PreviewShown/PreviewHidden is the answer; standalone, the
        // answer is whether the child we spawned is still alive — and nothing
        // told us when it exited, so ask now rather than trust what was true.
        if !mp_wm_api::hosted(cx) {
            self.preview.poll();
        }
        let open = self.preview.showing().is_some() || self.preview.hosted_showing().is_some();
        if open {
            self.preview.close(cx);
            self.set_preview_button(cx, false);
            self.status(cx, "Preview closed");
            return;
        }
        let Some(entry) = self
            .with_contents(cx, |contents, _| contents.selected_entry())
            .flatten()
        else {
            self.status(cx, "Select a file first — Space previews it");
            return;
        };
        if entry.is_dir {
            self.status(cx, &format!("{} is a folder — Enter opens it", entry.name));
            return;
        }
        match self.preview.open(cx, &entry.path) {
            Preview::Shown(message) => {
                // Hosted, the button lights when `PreviewShown` arrives, not
                // because we asked — see `handle_wm_event`.
                self.set_preview_button(cx, self.preview.showing().is_some());
                self.status(cx, &message);
            }
            // No viewer binary to show it: text and code still have a panel
            // here, which is better than nothing happening on Space.
            Preview::NoViewer(message) => {
                if entry.kind.is_textual() {
                    self.open_quick_look(cx, &entry);
                } else {
                    self.status(cx, &message);
                }
            }
        }
    }

    /// The in-app quick look: the head of a text file, monospaced.
    fn open_quick_look(&mut self, cx: &mut Cx, entry: &FileEntry) {
        let text = match model::read_head(&entry.path, 200, 512 * 1024) {
            Ok(text) => text,
            Err(error) => format!("Could not read {}:\n{}", entry.name, error),
        };
        self.ui.label(cx, ids!(ql_title)).set_text(cx, &entry.name);
        self.ui.label(cx, ids!(ql_text)).set_text(cx, &text);
        self.ui.widget(cx, ids!(quick_look)).set_visible(cx, true);
        self.quick_look_open = true;
        self.set_preview_button(cx, true);
        self.status(
            cx,
            &format!("Previewing {} — Space or Esc to close", entry.name),
        );
        self.ui.redraw(cx);
    }

    fn close_preview(&mut self, cx: &mut Cx) {
        self.preview.close(cx);
        if self.quick_look_open {
            self.quick_look_open = false;
            self.ui.widget(cx, ids!(quick_look)).set_visible(cx, false);
            self.ui.redraw(cx);
        }
        self.set_preview_button(cx, false);
        self.report(cx);
    }

    fn set_preview_button(&mut self, cx: &mut Cx, on: bool) {
        self.ui
            .widget(cx, ids!(preview_button))
            .widget(cx, ids!(btn_sel))
            .set_visible(cx, on);
    }

    /// The column picker: which columns the list view shows. The grid does
    /// not deliver right-clicks, so the menu button carries it.
    fn set_column_menu(&mut self, cx: &mut Cx, open: bool) {
        self.column_menu_open = open;
        self.ui.widget(cx, ids!(column_menu)).set_visible(cx, open);
        if open {
            self.refresh_column_menu(cx);
        }
        self.ui.redraw(cx);
    }

    fn refresh_column_menu(&mut self, cx: &mut Cx) {
        let shown = self
            .with_contents(cx, |contents, _| contents.columns())
            .unwrap_or_default();
        for (id, column) in COLUMN_ROWS {
            let on = shown.contains(&column);
            let mut row = self.ui.view(cx, id);
            row.widget(cx, ids!(menu_check)).set_visible(cx, on);
            row.widget(cx, ids!(menu_gap)).set_visible(cx, !on);
            let text = column.label().to_string();
            let color = if on {
                Palette::vec4(&Palette::shared().sel)
            } else {
                Palette::vec4(&Palette::shared().bg_dark)
            };
            script_apply_eval!(cx, row, {
                draw_bg +: {color: #(color)}
            });
            row.label(cx, ids!(menu_label)).set_text(cx, &text);
        }
    }

    /// The window manager's side of the conversation.
    fn handle_wm_event(&mut self, cx: &mut Cx, event: &mp_wm_api::WmEvent) {
        if !self.preview.on_wm_event(event) {
            return;
        }
        let showing = self.preview.hosted_showing().is_some();
        self.set_preview_button(cx, showing);
        if !showing {
            self.report(cx);
        }
    }


    // ------------------------------------------------------- context menu

    /// Open the menu at `at`. `entry` is what the press landed on; `None`
    /// means the empty space, which is a menu about the folder itself.
    fn open_menu(&mut self, cx: &mut Cx, at: DVec2, entry: Option<FileEntry>) {
        let rows = match &entry {
            Some(entry) => {
                let count = self
                    .with_contents(cx, |contents, _| contents.selection_count())
                    .unwrap_or(1)
                    .max(1);
                menu::entry_menu(count, entry.is_dir)
            }
            None => {
                let mode = self.tabs[self.tab].mode;
                menu::empty_menu(mode, self.clipboard.len(), self.show_hidden)
            }
        };
        self.menu_target = entry;
        self.menu_rows = rows;
        self.menu_hover = None;
        self.menu_open = true;
        self.close_submenu(cx);
        self.fill_menu(cx);
        self.place_menu(cx, ids!(context_menu), at, self.menu_rows.len(), &self.menu_rows.clone());
        self.ui.widget(cx, ids!(context_menu)).set_visible(cx, true);
        self.ui.redraw(cx);
    }

    fn close_menu(&mut self, cx: &mut Cx) {
        if !self.menu_open {
            return;
        }
        self.menu_open = false;
        self.menu_rows.clear();
        self.menu_target = None;
        self.menu_hover = None;
        self.ui.widget(cx, ids!(context_menu)).set_visible(cx, false);
        self.close_submenu(cx);
        self.ui.redraw(cx);
    }

    fn close_submenu(&mut self, cx: &mut Cx) {
        self.submenu_open = false;
        self.submenu_hover = None;
        self.ui
            .widget(cx, ids!(context_submenu))
            .set_visible(cx, false);
    }

    /// Push a card's top-left corner to `at`, kept inside the window. The
    /// overlay fills the window and its padding is the position — which is
    /// how a card gets placed without a custom layout pass.
    fn place_menu(&mut self, cx: &mut Cx, overlay: &[LiveId], at: DVec2, count: usize, rows: &[MenuRow]) {
        // The padding is measured from the overlay's own corner, and the
        // overlay starts under the window's caption bar — a press is in window
        // coordinates, so the difference has to come off. The measurement is
        // taken from the app's background, which shares the overlay's origin
        // and, unlike the overlay, has always been drawn.
        let host = self.ui.view(cx, ids!(app_bg)).area().rect(cx);
        let separators = rows.iter().take(count).filter(|r| r.separator).count() as f64;
        let height = count as f64 * CTX_ROW_H + separators * CTX_SEP_H + 10.0;
        let left = (at.x - host.pos.x)
            .min(host.size.x - CTX_PANEL_W - 6.0)
            .max(0.0);
        let top = (at.y - host.pos.y).min(host.size.y - height - 6.0).max(0.0);
        // The overlay fills the window and its padding is the card's corner.
        // Set component by component: `Inset` is a name the widget prelude
        // brings in, and a runtime `script_apply_eval!` has no prelude.
        let mut panel = self.ui.view(cx, overlay);
        script_apply_eval!(cx, panel, {
            padding.left: #(left)
            padding.top: #(top)
        });
    }

    /// Paint the rows the menu is currently offering into the slots.
    fn fill_menu(&mut self, cx: &mut Cx) {
        let rows = self.menu_rows.clone();
        let palette = Palette::shared();
        for (index, id) in CTX_IDS.iter().enumerate() {
            let slot = self.ui.view(cx, *id);
            let Some(row) = rows.get(index) else {
                slot.set_visible(cx, false);
                continue;
            };
            slot.set_visible(cx, true);
            slot.widget(cx, ids!(ctx_line))
                .set_visible(cx, row.separator);
            let label = if row.submenu {
                format!("{}  ›", row.label)
            } else {
                row.label.clone()
            };
            slot.label(cx, ids!(ctx_label)).set_text(cx, &label);
            slot.label(cx, ids!(ctx_hint)).set_text(cx, row.hint);
            let hovered = self.menu_hover == Some(index);
            let bg = if hovered {
                Palette::vec4(&palette.hover_soft)
            } else {
                Palette::vec4(&palette.bg)
            };
            let text = if row.danger {
                Palette::vec4(&palette.danger)
            } else if hovered {
                Palette::vec4(&palette.accent)
            } else {
                Palette::vec4(&palette.fg)
            };
            let mut body = slot.view(cx, ids!(ctx_body));
            script_apply_eval!(cx, body, {
                draw_bg +: {color: #(bg)}
            });
            let mut label_widget = slot.label(cx, ids!(ctx_label));
            script_apply_eval!(cx, label_widget, {
                draw_text +: {color: #(text)}
            });
        }
    }

    fn fill_submenu(&mut self, cx: &mut Cx) {
        let apps = self.submenu_apps.clone();
        let palette = Palette::shared();
        for (index, id) in CTX_SUB_IDS.iter().enumerate() {
            let slot = self.ui.view(cx, *id);
            let Some(app) = apps.get(index) else {
                slot.set_visible(cx, false);
                continue;
            };
            slot.set_visible(cx, true);
            slot.widget(cx, ids!(ctx_line)).set_visible(cx, false);
            slot.label(cx, ids!(ctx_label)).set_text(cx, &app.label);
            slot.label(cx, ids!(ctx_hint)).set_text(cx, "");
            let hovered = self.submenu_hover == Some(index);
            let bg = if hovered {
                Palette::vec4(&palette.hover_soft)
            } else {
                Palette::vec4(&palette.bg)
            };
            let text = if hovered {
                Palette::vec4(&palette.accent)
            } else {
                Palette::vec4(&palette.fg)
            };
            let mut body = slot.view(cx, ids!(ctx_body));
            script_apply_eval!(cx, body, {
                draw_bg +: {color: #(bg)}
            });
            let mut label_widget = slot.label(cx, ids!(ctx_label));
            script_apply_eval!(cx, label_widget, {
                draw_text +: {color: #(text)}
            });
        }
    }

    /// Open the Open With list beside its row.
    fn open_submenu(&mut self, cx: &mut Cx, row: usize) {
        let Some(entry) = self.menu_target.clone() else {
            return;
        };
        let available = |app: &str| preview::app_available(cx, app);
        self.submenu_apps = menu::open_with_apps(&entry.path, &available)
            .into_iter()
            .map(|(id, label)| AppChoice { id, label })
            .collect();
        if self.submenu_apps.is_empty() {
            return;
        }
        self.submenu_open = true;
        self.submenu_hover = None;
        self.fill_submenu(cx);
        // Beside the row it belongs to, so the eye does not have to look for
        // it: the card's own left edge plus its width, at the row's height.
        let card = self.ui.view(cx, ids!(ctx_panel)).area().rect(cx);
        let separators = self.menu_rows[..row].iter().filter(|r| r.separator).count() as f64;
        let at = dvec2(
            card.pos.x + CTX_PANEL_W - 8.0,
            card.pos.y + 5.0 + row as f64 * CTX_ROW_H + separators * CTX_SEP_H,
        );
        let rows: Vec<MenuRow> = Vec::new();
        self.place_menu(cx, ids!(context_submenu), at, self.submenu_apps.len(), &rows);
        self.ui
            .widget(cx, ids!(context_submenu))
            .set_visible(cx, true);
        self.ui.redraw(cx);
    }

    /// Do what a row says. Every arm is a thing this app already does — a row
    /// with nothing behind it would be a lie, so there are none.
    fn fire_menu(&mut self, cx: &mut Cx, action: MenuAction) {
        let target = self.menu_target.clone();
        self.close_menu(cx);
        match action {
            MenuAction::Open => {
                if let Some(entry) = target {
                    self.open_entry(cx, entry);
                }
            }
            MenuAction::OpenWith => {}
            MenuAction::Preview => self.toggle_preview(cx),
            MenuAction::NewFolder => self.new_folder(cx),
            MenuAction::Rename => self.begin_rename(cx),
            MenuAction::Duplicate => self.duplicate(cx),
            MenuAction::Copy => self.copy_selection(cx, false),
            MenuAction::Cut => self.copy_selection(cx, true),
            MenuAction::Paste => self.paste(cx),
            MenuAction::SelectAll => {
                self.with_contents(cx, |contents, cx| contents.select_all(cx));
                self.report(cx);
            }
            MenuAction::Trash => self.trash_selection(cx),
            MenuAction::DeleteForever => self.delete_forever(cx),
            MenuAction::RevealInTreemap => self.reveal_in_treemap(cx, target),
            MenuAction::Properties => self.set_props(cx, true),
            MenuAction::OpenInTerminal => self.open_terminal(cx),
            MenuAction::ShowHidden => self.toggle_hidden(cx),
            MenuAction::SetMode(mode) => self.set_mode(cx, mode),
            MenuAction::OpenWithApp(index) => {
                let (Some(entry), Some(app)) = (target, self.submenu_apps.get(index).cloned())
                else {
                    return;
                };
                let message = preview::open_file_with(cx, &entry.path, &app.id);
                self.status(cx, &message);
            }
        }
    }

    /// Copy the selection into the folder it is already in — which the
    /// collision rule turns into "name (2)".
    fn duplicate(&mut self, cx: &mut Cx) {
        let paths: Vec<PathBuf> = self
            .with_contents(cx, |contents, _| contents.selected_entries())
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.path)
            .collect();
        if paths.is_empty() {
            self.status(cx, "Nothing selected to duplicate");
            return;
        }
        self.submit(cx, OpKind::Copy, paths, None);
    }

    /// Show the entry on the map. The map is of the folder we are in, so this
    /// is a view change plus a highlight — not a search.
    fn reveal_in_treemap(&mut self, cx: &mut Cx, entry: Option<FileEntry>) {
        self.set_mode(cx, ViewMode::Treemap);
        let Some(entry) = entry else {
            return;
        };
        let name = entry.name.clone();
        let map = self.with_contents(cx, |contents, cx| contents.treemap(cx));
        if let Some(map) = map {
            map.set_selected(cx, Some(entry.path));
        }
        self.status(cx, &format!("{name} is highlighted on the map"));
    }

    /// Erase, with nothing behind it. Asked once in the status bar and done on
    /// the second press: there is no undo for this, so a single slip must not
    /// be enough.
    fn delete_forever(&mut self, cx: &mut Cx) {
        let paths: Vec<PathBuf> = self
            .with_contents(cx, |contents, _| contents.selected_entries())
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.path)
            .collect();
        if paths.is_empty() {
            self.status(cx, "Nothing selected to delete");
            return;
        }
        if self.pending_delete != paths {
            let count = paths.len();
            self.pending_delete = paths;
            self.status(
                cx,
                &format!(
                    "Delete {count} item{} permanently? This cannot be undone — press Shift+Delete again to confirm, Esc to cancel",
                    if count == 1 { "" } else { "s" }
                ),
            );
            return;
        }
        self.pending_delete.clear();
        self.submit(cx, OpKind::Delete, paths, None);
    }

    fn toggle_hidden(&mut self, cx: &mut Cx) {
        self.show_hidden = !self.show_hidden;
        self.ui.label(cx, ids!(hidden_hint)).set_text(
            cx,
            if self.show_hidden {
                "Ctrl+H  Hide hidden files"
            } else {
                "Ctrl+H  Show hidden files"
            },
        );
        self.request_directory(cx);
    }

    // ---------------------------------------------------------- properties

    fn set_props(&mut self, cx: &mut Cx, open: bool) {
        self.props_open = open;
        self.ui.widget(cx, ids!(props_panel)).set_visible(cx, open);
        self.ui
            .widget(cx, ids!(props_button))
            .widget(cx, ids!(btn_sel))
            .set_visible(cx, open);
        if open {
            self.refresh_props(cx);
        } else if let Some(cancel) = self.size_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.ui.redraw(cx);
    }

    fn set_prop(&mut self, cx: &mut Cx, id: &[LiveId], value: &str) {
        let row = self.ui.view(cx, id);
        row.label(cx, ids!(prop_value)).set_text(cx, value);
    }

    fn refresh_props(&mut self, cx: &mut Cx) {
        if !self.props_open {
            return;
        }
        let picked = self
            .with_contents(cx, |contents, _| contents.selected_entries())
            .unwrap_or_default();
        // More than one thing selected has no single name or kind, so the
        // panel describes the set instead of pretending.
        if picked.len() > 1 {
            let bytes: u64 = picked.iter().map(|e| e.size).sum();
            self.set_prop(cx, ids!(prop_name), &format!("{} items", picked.len()));
            self.set_prop(cx, ids!(prop_kind), "Multiple selection");
            self.set_prop(
                cx,
                ids!(prop_size),
                &format!("{} of files (folders not counted)", model::format_size(bytes, false)),
            );
            for id in [
                ids!(prop_modified),
                ids!(prop_created),
                ids!(prop_permissions),
                ids!(prop_opens),
            ] {
                self.set_prop(cx, id, "—");
            }
            let dir = self.current_dir().display().to_string();
            self.set_prop(cx, ids!(prop_path), &dir);
            self.ui.widget(cx, ids!(prop_spinner)).set_visible(cx, false);
            return;
        }
        // Nothing selected describes the folder itself, which is what Cmd+I
        // on an empty selection means in Files.
        let entry = picked.into_iter().next();
        let (path, name, kind, modified, created, permissions, is_dir, size) = match &entry {
            Some(e) => (
                e.path.clone(),
                e.name.clone(),
                e.kind_text(),
                e.modified_text(),
                e.created_text(),
                e.permissions.clone(),
                e.is_dir,
                e.size,
            ),
            None => {
                // Nothing selected describes the folder itself, and the honest
                // way to describe it is to ask its own parent for its entry —
                // the same listing every other row here comes from.
                let dir = self.current_dir();
                let entry = dir.parent().and_then(|parent| {
                    vfs()
                        .read_dir(parent, true)
                        .ok()?
                        .into_iter()
                        .find(|e| e.path == dir)
                });
                match entry {
                    Some(e) => (
                        dir,
                        e.name.clone(),
                        e.kind_text(),
                        e.modified_text(),
                        e.created_text(),
                        e.permissions.clone(),
                        true,
                        0,
                    ),
                    None => (
                        dir.clone(),
                        display_name(&dir),
                        "Folder".to_string(),
                        "—".to_string(),
                        "—".to_string(),
                        "—".to_string(),
                        true,
                        0,
                    ),
                }
            }
        };
        self.set_prop(cx, ids!(prop_name), &name);
        self.set_prop(cx, ids!(prop_kind), &kind);
        // The date says when; the age says whether that is recent, which is
        // the question anyone actually has about a file.
        let now = model::now_secs();
        let age = entry
            .as_ref()
            .filter(|e| e.modified_secs > 0)
            .map(|e| format!("   {}", model::format_age(e.modified_secs, now)))
            .unwrap_or_default();
        self.set_prop(cx, ids!(prop_modified), &format!("{modified}{age}"));
        self.set_prop(cx, ids!(prop_created), &created);
        self.set_prop(
            cx,
            ids!(prop_permissions),
            &format!("{}  {}", octal_mode(&path), permissions),
        );
        let where_text = path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| path.display().to_string());
        self.set_prop(cx, ids!(prop_path), &where_text);
        self.set_prop(
            cx,
            ids!(prop_opens),
            if is_dir {
                "mpfiles"
            } else {
                mp_wm_api::viewer_for(&path)
            },
        );
        if !is_dir {
            self.ui.widget(cx, ids!(prop_spinner)).set_visible(cx, false);
            let text = format!("{} ({} bytes)", model::format_size(size, false), size);
            self.set_prop(cx, ids!(prop_size), &text);
            return;
        }
        // A folder's size is a whole recursive walk: it goes on a thread, and
        // the panel spins until it lands.
        self.set_prop(cx, ids!(prop_size), "Measuring…");
        self.ui.widget(cx, ids!(prop_spinner)).set_visible(cx, true);
        self.measure_folder(cx, path);
    }

    fn measure_folder(&mut self, cx: &mut Cx, path: PathBuf) {
        if let Some(cancel) = self.size_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        let Some(sender) = self.size_sender.clone() else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.size_cancel = Some(cancel.clone());
        let _ = cx;
        thread::spawn(move || {
            let bytes = vfs().total_bytes(&path, &cancel);
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            if sender.send(SizeResult { path, bytes }).is_ok() {
                SignalToUI::set_ui_signal();
            }
        });
    }

    fn drain_sizes(&mut self, cx: &mut Cx) {
        let results: Vec<SizeResult> = self
            .size_receiver
            .as_ref()
            .map(|r| r.try_iter().collect())
            .unwrap_or_default();
        for result in results {
            // The panel may have moved on to another folder while the walk
            // ran; a stale total is worse than no total.
            let showing = self
                .with_contents(cx, |contents, _| contents.selected_entry())
                .flatten()
                .map(|e| e.path)
                .unwrap_or_else(|| self.current_dir());
            if !self.props_open || showing != result.path {
                continue;
            }
            self.ui.widget(cx, ids!(prop_spinner)).set_visible(cx, false);
            let text = format!(
                "{} ({} bytes)",
                model::format_size(result.bytes, false),
                result.bytes
            );
            self.set_prop(cx, ids!(prop_size), &text);
        }
    }

    // ------------------------------------------------------ file operations

    fn next_op_id(&mut self) -> u64 {
        self.op_id = self.op_id.wrapping_add(1);
        self.op_id
    }

    fn submit(&mut self, cx: &mut Cx, kind: OpKind, sources: Vec<PathBuf>, new_name: Option<String>) {
        let id = self.next_op_id();
        let request = OpRequest {
            id,
            kind,
            sources,
            dest_dir: self.current_dir(),
            new_name,
            home: self.home.clone(),
        };
        // An in-memory tree changes at once: sending it to a worker would only
        // buy a progress bar for work that is already finished.
        if vfs().is_instant() {
            let update = match vfs().perform(&request) {
                Ok(outcome) => OpUpdate::Done {
                    id,
                    kind,
                    message: outcome.message,
                    undo: outcome.undo,
                    touched: outcome.touched,
                },
                Err(message) => OpUpdate::Failed { id, kind, message },
            };
            self.apply_op_update(cx, update);
            return;
        }
        let Some(engine) = self.ops.as_ref() else {
            return;
        };
        engine.submit(request);
        self.active_op = Some(id);
        self.show_progress(cx, true, 0.0, &format!("{}…", kind.verb()));
    }

    fn show_progress(&mut self, cx: &mut Cx, on: bool, fraction: f64, text: &str) {
        self.ui.widget(cx, ids!(progress_row)).set_visible(cx, on);
        if on {
            self.ui.label(cx, ids!(progress_label)).set_text(cx, text);
            let width = (fraction.clamp(0.0, 1.0) * 150.0).round();
            let mut fill = self.ui.view(cx, ids!(progress_fill));
            script_apply_eval!(cx, fill, {
                width: #(width)
            });
        }
        self.ui.redraw(cx);
    }

    /// Put the progress row away once the engine has nothing left to do. A
    /// batch rename is many jobs; hiding the row on the first `Done` would
    /// make the rest of them invisible.
    fn finish_op(&mut self, cx: &mut Cx) {
        let busy = self.ops.as_ref().map(|engine| engine.busy()).unwrap_or(false);
        if !busy {
            self.show_progress(cx, false, 0.0, "");
        }
    }

    fn drain_ops(&mut self, cx: &mut Cx) {
        let updates = self
            .ops
            .as_ref()
            .map(|engine| engine.drain())
            .unwrap_or_default();
        for update in updates {
            self.apply_op_update(cx, update);
        }
    }

    /// One finished (or progressing) operation, whichever filesystem did it.
    fn apply_op_update(&mut self, cx: &mut Cx, update: OpUpdate) {
        {
            match update {
                OpUpdate::Progress {
                    id,
                    kind,
                    done,
                    total,
                    current,
                } => {
                    if self.active_op != Some(id) {
                        return;
                    }
                    let fraction = if total == 0 {
                        0.0
                    } else {
                        done as f64 / total as f64
                    };
                    let text = format!(
                        "{} {} — {} of {}",
                        kind.verb(),
                        current,
                        model::format_size(done, false),
                        model::format_size(total, false)
                    );
                    self.show_progress(cx, true, fraction, &text);
                }
                OpUpdate::Done {
                    id,
                    kind,
                    message,
                    undo,
                    touched,
                } => {
                    if let Some(undo) = undo {
                        self.journal.push(undo);
                    }
                    if self.active_op == Some(id) {
                        self.active_op = None;
                    }
                    self.finish_op(cx);
                    // What a job left behind is worth selecting only when it
                    // landed *here*: a trashed file's `touched` path is inside
                    // the Trash, and selecting it would select nothing.
                    if kind != OpKind::Trash {
                        self.pending_select = touched;
                    }
                    self.status(cx, &message);
                    self.pending_status = Some(message);
                    self.request_directory(cx);
                }
                OpUpdate::Failed { id, kind, message } => {
                    if self.active_op == Some(id) {
                        self.active_op = None;
                    }
                    self.finish_op(cx);
                    let text = format!("{} failed — {message}", kind.verb());
                    self.status(cx, &text);
                    self.pending_status = Some(text);
                    self.request_directory(cx);
                }
            }
        }
    }

    fn copy_selection(&mut self, cx: &mut Cx, cut: bool) {
        let paths: Vec<PathBuf> = self
            .with_contents(cx, |contents, _| contents.selected_entries())
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.path)
            .collect();
        if paths.is_empty() {
            self.status(cx, "Nothing selected to copy");
            return;
        }
        let count = paths.len();
        self.clipboard = paths;
        self.clipboard_cut = cut;
        self.status(
            cx,
            &format!(
                "{} {} item{} — Cmd+V pastes {} here or in another tab",
                if cut { "Cut" } else { "Copied" },
                count,
                if count == 1 { "" } else { "s" },
                if count == 1 { "it" } else { "them" }
            ),
        );
    }

    fn paste(&mut self, cx: &mut Cx) {
        if self.clipboard.is_empty() {
            self.status(cx, "The clipboard is empty");
            return;
        }
        let sources = self.clipboard.clone();
        let kind = if self.clipboard_cut {
            OpKind::Move
        } else {
            OpKind::Copy
        };
        // A cut is spent once it is pasted; a copy stays on the clipboard the
        // way it does everywhere else.
        if self.clipboard_cut {
            self.clipboard.clear();
        }
        self.submit(cx, kind, sources, None);
    }

    fn trash_selection(&mut self, cx: &mut Cx) {
        let paths: Vec<PathBuf> = self
            .with_contents(cx, |contents, _| contents.selected_entries())
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.path)
            .collect();
        if paths.is_empty() {
            self.status(cx, "Nothing selected to move to the Trash");
            return;
        }
        self.submit(cx, OpKind::Trash, paths, None);
    }

    fn new_folder(&mut self, cx: &mut Cx) {
        let name = ops::unique_path(&self.current_dir(), "untitled folder");
        let name = display_name(&name);
        // The new folder arrives selected with its name up for editing, which
        // is the only moment anyone ever wants to type a folder name.
        self.pending_rename = Some(self.current_dir().join(&name));
        self.submit(cx, OpKind::NewFolder, Vec::new(), Some(name));
    }

    fn undo(&mut self, cx: &mut Cx) {
        let Some(undo) = self.journal.pop() else {
            self.status(cx, "Nothing to undo");
            return;
        };
        let id = self.next_op_id();
        let home = self.home.clone();
        let description = undo.describe();
        if vfs().is_instant() {
            let update = match vfs().perform_undo(&undo) {
                Ok(outcome) => OpUpdate::Done {
                    id,
                    kind: OpKind::Move,
                    message: outcome.message,
                    undo: None,
                    touched: outcome.touched,
                },
                Err(message) => OpUpdate::Failed {
                    id,
                    kind: OpKind::Move,
                    message,
                },
            };
            self.apply_op_update(cx, update);
            self.status(cx, &description);
            return;
        }
        let Some(engine) = self.ops.as_ref() else {
            return;
        };
        engine.submit_undo(id, undo, home);
        self.active_op = Some(id);
        self.show_progress(cx, true, 0.0, &description);
        let left = self.journal.len();
        self.status(
            cx,
            &format!(
                "{description} — {left} step{} left to undo",
                if left == 1 { "" } else { "s" }
            ),
        );
    }

    // -------------------------------------------------------------- rename

    fn begin_rename(&mut self, cx: &mut Cx) {
        let picked = self
            .with_contents(cx, |contents, _| contents.selected_entries())
            .unwrap_or_default();
        match picked.len() {
            0 => self.status(cx, "Select a file first — F2 renames it"),
            1 => {
                let path = picked[0].path.clone();
                if self.tabs[self.tab].mode == ViewMode::Treemap {
                    self.status(cx, "Renaming needs a list view — Cmd+1, 2 or 3");
                    return;
                }
                self.with_contents(cx, |contents, cx| contents.begin_rename(cx, &path));
                self.status(cx, "Type the new name, Enter to rename, Esc to leave it");
            }
            _ => self.open_batch(cx, picked.into_iter().map(|e| e.path).collect()),
        }
    }

    fn commit_rename(&mut self, cx: &mut Cx, path: PathBuf, name: String) {
        if let Some(problem) = rename::name_error(&name) {
            self.status(cx, problem);
            return;
        }
        if name == display_name(&path) {
            self.report(cx);
            return;
        }
        self.pending_select = vec![path.with_file_name(&name)];
        self.submit(cx, OpKind::Rename, vec![path], Some(name));
    }

    // -------------------------------------------------------- batch rename

    fn open_batch(&mut self, cx: &mut Cx, targets: Vec<PathBuf>) {
        self.batch_targets = targets;
        self.batch_open = true;
        let count = self.batch_targets.len();
        self.ui.label(cx, ids!(batch_title)).set_text(
            cx,
            &format!("Rename {} item{}", count, if count == 1 { "" } else { "s" }),
        );
        for id in [ids!(batch_find), ids!(batch_replace), ids!(batch_pattern)] {
            let row = self.ui.view(cx, id);
            row.text_input(cx, ids!(field_input)).set_text(cx, "");
        }
        self.ui.widget(cx, ids!(batch_dialog)).set_visible(cx, true);
        self.refresh_batch_preview(cx);
        self.focus_soon(cx, FocusTarget::Batch);
        self.ui.redraw(cx);
    }

    fn close_batch(&mut self, cx: &mut Cx) {
        self.batch_open = false;
        self.batch_targets.clear();
        self.ui.widget(cx, ids!(batch_dialog)).set_visible(cx, false);
        cx.set_key_focus(Area::Empty);
        self.report(cx);
        self.ui.redraw(cx);
    }

    fn batch_field(&mut self, cx: &mut Cx, id: &[LiveId]) -> String {
        self.ui.view(cx, id).text_input(cx, ids!(field_input)).text()
    }

    /// What the dialog's fields would do, computed by the same function that
    /// will do it — so the preview cannot drift from the result.
    fn batch_plan(&mut self, cx: &mut Cx) -> Vec<(PathBuf, String)> {
        let find = self.batch_field(cx, ids!(batch_find));
        let replace = self.batch_field(cx, ids!(batch_replace));
        let pattern = self.batch_field(cx, ids!(batch_pattern));
        let mode = BatchMode::from_fields(&find, &replace, &pattern);
        let names: Vec<String> = self
            .batch_targets
            .iter()
            .map(|p| display_name(p))
            .collect();
        let renamed = rename::batch_rename(&names, &mode, 1);
        if rename::is_noop(&names, &renamed) {
            return Vec::new();
        }
        self.batch_targets
            .iter()
            .cloned()
            .zip(renamed)
            .filter(|(path, name)| &display_name(path) != name)
            .collect()
    }

    fn refresh_batch_preview(&mut self, cx: &mut Cx) {
        let plan = self.batch_plan(cx);
        let text = if plan.is_empty() {
            "Nothing would change yet.".to_string()
        } else {
            let mut lines: Vec<String> = plan
                .iter()
                .take(6)
                .map(|(path, name)| format!("{}  →  {}", display_name(path), name))
                .collect();
            if plan.len() > lines.len() {
                lines.push(format!("…and {} more", plan.len() - lines.len()));
            }
            lines.join("\n")
        };
        self.ui.label(cx, ids!(batch_preview)).set_text(cx, &text);
    }

    fn apply_batch(&mut self, cx: &mut Cx) {
        let plan = self.batch_plan(cx);
        if plan.is_empty() {
            self.status(cx, "That pattern would not change any name");
            return;
        }
        let count = plan.len();
        self.close_batch(cx);
        // One rename job per file: the engine's Rename takes a single source,
        // and a failure on one name must not abandon the rest.
        let mut selected = Vec::with_capacity(count);
        for (path, name) in plan {
            selected.push(path.with_file_name(&name));
            self.submit(cx, OpKind::Rename, vec![path], Some(name));
        }
        self.pending_select = selected;
        self.status(cx, &format!("Renaming {count} items"));
    }

    // ------------------------------------------------------------ terminal

    fn open_terminal(&mut self, cx: &mut Cx) {
        let dir = self.current_dir();
        let request = mp_wm_api::WmRequest::Launch {
            app: "terminal".to_string(),
            args: vec!["--cwd".to_string(), dir.display().to_string()],
        };
        if mp_wm_api::send(cx, &request) {
            self.status(cx, &format!("Opening a terminal in {}", dir.display()));
            return;
        }
        let Some(bin) = preview::sibling_bin("mpterm") else {
            self.status(cx, "mpterm is not built — nothing to open a terminal with");
            return;
        };
        match Command::new(&bin).arg("--cwd").arg(&dir).spawn() {
            Ok(_) => self.status(cx, &format!("Opening a terminal in {}", dir.display())),
            Err(error) => self.status(cx, &format!("Could not start mpterm: {error}")),
        }
    }

    // ---------------------------------------------------------------- open

    fn open_entry(&mut self, cx: &mut Cx, entry: FileEntry) {
        if entry.is_dir {
            self.navigate(cx, entry.path, true);
            return;
        }
        let message = preview::open_file(cx, &entry.path);
        self.status(cx, &message);
    }

    fn describe(&mut self, cx: &mut Cx, entry: &FileEntry) {
        let picked = self
            .with_contents(cx, |contents, _| contents.selection_count())
            .unwrap_or(1);
        if picked > 1 {
            self.report(cx);
            self.refresh_props(cx);
            return;
        }
        // An open Quick Look panel follows the selection, the way Finder's
        // does: the same viewer is retargeted, so arrow keys dial through
        // previews without this window ever losing the keyboard.
        self.preview.retarget(cx, &entry.path);
        let text = if entry.is_dir {
            format!("{} — folder", entry.name)
        } else {
            format!(
                "{} — {} — {} — {} — opens with {}",
                entry.name,
                entry.kind.label(),
                model::format_size(entry.size, false),
                entry.modified_text(),
                mp_wm_api::viewer_for(&entry.path),
            )
        };
        self.status(cx, &text);
        self.refresh_props(cx);
    }

    // ------------------------------------------------------------ keyboard

    fn handle_key(&mut self, cx: &mut Cx, event: &KeyEvent) {
        let command = event.modifiers.control || event.modifiers.logo;
        let shift = event.modifiers.shift;

        // Escape unwinds whatever is on top, innermost first.
        if event.key_code == KeyCode::Escape {
            if self.menu_open {
                return self.close_menu(cx);
            }
            if !self.pending_delete.is_empty() {
                self.pending_delete.clear();
                self.status(cx, "Nothing was deleted");
                return;
            }
            if self.batch_open {
                return self.close_batch(cx);
            }
            if self.column_menu_open {
                return self.set_column_menu(cx, false);
            }
            if self.path_edit_open {
                return self.set_path_edit(cx, false);
            }
            if self
                .with_contents(cx, |contents, _| contents.is_renaming())
                .unwrap_or(false)
            {
                self.with_contents(cx, |contents, cx| contents.cancel_rename(cx));
                return self.report(cx);
            }
            if self.quick_look_open || self.preview.showing().is_some() {
                self.close_preview(cx);
            } else if self.search_visible {
                self.set_search(cx, false);
            } else {
                self.with_contents(cx, |contents, cx| contents.clear_selection(cx));
                self.refresh_props(cx);
                self.report(cx);
            }
            return;
        }

        // Every text field in this window keeps key focus while it is hidden,
        // so what is *open* decides whether a key is text or navigation.
        let editing = self.batch_open
            || self.path_edit_open
            || self.search_visible
            || self
                .with_contents(cx, |contents, _| contents.is_renaming())
                .unwrap_or(false);

        if command {
            match event.key_code {
                KeyCode::KeyT if !shift => return self.new_tab(cx),
                KeyCode::KeyW => return self.close_tab(cx),
                KeyCode::LBracket if shift => return self.switch_tab(cx, -1),
                KeyCode::RBracket if shift => return self.switch_tab(cx, 1),
                KeyCode::KeyL => return self.set_path_edit(cx, !self.path_edit_open),
                KeyCode::KeyD if !editing => return self.bookmark_current(cx),
                KeyCode::KeyN if shift => return self.new_folder(cx),
                KeyCode::KeyI if !editing => {
                    let open = !self.props_open;
                    return self.set_props(cx, open);
                }
                KeyCode::KeyZ if !editing => return self.undo(cx),
                KeyCode::KeyC if !editing => return self.copy_selection(cx, false),
                KeyCode::KeyX if !editing => return self.copy_selection(cx, true),
                KeyCode::KeyV if !editing => return self.paste(cx),
                KeyCode::KeyA if !editing => {
                    self.with_contents(cx, |contents, cx| contents.select_all(cx));
                    self.report(cx);
                    return;
                }
                KeyCode::Backspace if !editing => return self.trash_selection(cx),
                KeyCode::Equals | KeyCode::NumpadAdd if !editing => return self.zoom(cx, 1),
                KeyCode::Minus | KeyCode::NumpadSubtract if !editing => return self.zoom(cx, -1),
                KeyCode::KeyH => return self.toggle_hidden(cx),
                KeyCode::KeyF => {
                    self.set_search(cx, !self.search_visible);
                    return;
                }
                KeyCode::Key1 => return self.set_mode(cx, ViewMode::Icons),
                KeyCode::Key2 => return self.set_mode(cx, ViewMode::List),
                KeyCode::Key3 => return self.set_mode(cx, ViewMode::Compact),
                KeyCode::Key4 => return self.set_mode(cx, ViewMode::Treemap),
                _ => {}
            }
        }
        // Ctrl+Tab cycles tabs even on macOS, where Cmd+Tab belongs to the OS.
        if event.key_code == KeyCode::Tab && event.modifiers.control {
            return self.switch_tab(cx, if shift { -1 } else { 1 });
        }
        if event.key_code == KeyCode::F2 && !editing {
            return self.begin_rename(cx);
        }
        if event.key_code == KeyCode::Delete && !editing {
            if shift {
                return self.delete_forever(cx);
            }
            return self.trash_selection(cx);
        }
        // The macOS keyboard's Delete key is Backspace, so the same pair holds
        // there: with Cmd it trashes, with Cmd+Shift it erases.
        if event.key_code == KeyCode::Backspace && command && shift && !editing {
            return self.delete_forever(cx);
        }
        if editing {
            return;
        }
        match event.key_code {
            KeyCode::Space => self.toggle_preview(cx),
            KeyCode::Backspace => self.go_up(cx),
            // Cmd/Ctrl+Up is the desktop's "open parent folder".
            KeyCode::ArrowUp if command => self.go_up(cx),
            KeyCode::ReturnKey | KeyCode::NumpadEnter => {
                if let Some(entry) = self
                    .with_contents(cx, |contents, _| contents.selected_entry())
                    .flatten()
                {
                    self.open_entry(cx, entry);
                }
            }
            KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::ArrowUp | KeyCode::ArrowDown => {
                let stride = self
                    .with_contents(cx, |contents, _| contents.row_stride())
                    .unwrap_or(1);
                let amount = match event.key_code {
                    KeyCode::ArrowLeft => -1,
                    KeyCode::ArrowRight => 1,
                    KeyCode::ArrowUp => -stride,
                    _ => stride,
                };
                let selected = self
                    .with_contents(cx, |contents, cx| contents.move_selection(cx, amount, shift))
                    .flatten();
                if let Some(entry) = selected {
                    self.describe(cx, &entry);
                }
            }
            _ => {}
        }
    }

    // -------------------------------------------------------------- events

    /// A drag ended at `at`: over the sidebar it means "bookmark this".
    fn handle_drop(&mut self, cx: &mut Cx, paths: Vec<PathBuf>, at: DVec2) {
        let rect = self.ui.view(cx, ids!(sidebar)).area().rect(cx);
        if !rect.contains(at) {
            return;
        }
        let folders: Vec<PathBuf> = paths.into_iter().filter(|p| vfs().is_dir(p)).collect();
        if folders.is_empty() {
            self.status(cx, "Only folders can be bookmarked");
            return;
        }
        for folder in folders {
            self.bookmark(cx, folder);
        }
    }

    fn handle_contents_action(&mut self, cx: &mut Cx, action: FileContentsAction) {
        match action {
            FileContentsAction::Open(entry) => self.open_entry(cx, entry),
            FileContentsAction::Selected(entry) => self.describe(cx, &entry),
            FileContentsAction::Sorted => self.report(cx),
            FileContentsAction::Renamed(path, name) => self.commit_rename(cx, path, name),
            FileContentsAction::RenameCancelled => self.report(cx),
            FileContentsAction::Dropped(paths, at) => self.handle_drop(cx, paths, at),
            FileContentsAction::NeedChildren(folder) => self.request_children(cx, folder),
            FileContentsAction::Context { at, entry } => self.open_menu(cx, at, entry),
            FileContentsAction::Drill(path) => {
                let folder = if vfs().is_dir(&path) {
                    path
                } else {
                    path.parent().map(Path::to_path_buf).unwrap_or_default()
                };
                if vfs().is_dir(&folder) {
                    self.navigate(cx, folder, true);
                }
            }
        }
    }
}

/// The mode as `755`, next to the `rwx` letters the listing already shows.
fn octal_mode(path: &Path) -> String {
    // A virtual file has no inode to ask, and inventing one would be a number
    // that means nothing.
    if vfs::is_demo() {
        return "—".to_string();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(meta) => format!("{:o}", meta.permissions().mode() & 0o7777),
            Err(_) => "—".to_string(),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        "—".to_string()
    }
}

impl App {
    /// The menu's own pointer handling. Returns true when the event was the
    /// menu's, so nothing behind it also acts on the same click.
    fn handle_menu_actions(&mut self, cx: &mut Cx, actions: &Actions) -> bool {
        // The submenu is on top, so it is asked first.
        if self.submenu_open {
            for index in 0..self.submenu_apps.len() {
                let row = self.ui.view(cx, CTX_SUB_IDS[index]).view(cx, ids!(ctx_body));
                if row.finger_hover_in(actions).is_some() && self.submenu_hover != Some(index) {
                    self.submenu_hover = Some(index);
                    self.fill_submenu(cx);
                    self.ui.redraw(cx);
                }
                if row.finger_down(actions).is_some() {
                    self.fire_menu(cx, MenuAction::OpenWithApp(index));
                    return true;
                }
            }
        }
        let rows = self.menu_rows.clone();
        for (index, row) in rows.iter().enumerate() {
            let body = self.ui.view(cx, CTX_IDS[index]).view(cx, ids!(ctx_body));
            if body.finger_hover_in(actions).is_some() && self.menu_hover != Some(index) {
                self.menu_hover = Some(index);
                self.fill_menu(cx);
                // Moving onto another row puts away a submenu that belonged to
                // the one before it.
                if row.action != MenuAction::OpenWith {
                    self.close_submenu(cx);
                } else {
                    self.open_submenu(cx, index);
                }
                self.ui.redraw(cx);
            }
            if body.finger_down(actions).is_some() {
                if row.action == MenuAction::OpenWith {
                    self.open_submenu(cx, index);
                } else {
                    self.fire_menu(cx, row.action);
                }
                return true;
            }
        }
        // Anywhere else closes it, which is what clicking outside a menu means.
        if self
            .ui
            .view(cx, ids!(context_menu))
            .finger_down(actions)
            .is_some()
        {
            self.close_menu(cx);
            return true;
        }
        false
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        // `--demo` browses a home that does not exist, so a screen recording
        // can show every feature of this app without showing anybody's disk.
        // It is chosen before anything reads a path, and never afterwards.
        if vfs::demo_requested() {
            vfs::install(Arc::new(demo::DemoVfs::new()));
        }
        let (sender, receiver) = mpsc::channel();
        self.sender = Some(sender);
        self.receiver = Some(receiver);
        let (size_sender, size_receiver) = mpsc::channel();
        self.size_sender = Some(size_sender);
        self.size_receiver = Some(size_receiver);
        self.ops = Some(Ops::new(Box::new(SignalToUI::set_ui_signal)));
        self.home = vfs().home();
        // The demo must not write to the real home, so its bookmarks live and
        // die with the window.
        self.bookmarks = if vfs::is_demo() {
            Bookmarks::in_memory(Vec::new())
        } else {
            Bookmarks::load(&self.home)
        };
        if vfs::is_demo() {
            // Say so where it cannot be missed: a recording of the demo must
            // never be mistaken for a recording of somebody's files.
            self.ui.label(cx, ids!(files_title)).set_text(cx, "Files · Demo");
        }
        let palette = Palette::shared();
        let colors = contents::Colors {
            dim: Palette::vec4(&palette.fg_dim),
            selection: Palette::vec4(&palette.sel),
        };
        self.with_contents(cx, |contents, cx| {
            contents.set_colors(cx, colors);
            contents.set_zoom(cx, DEFAULT_ZOOM);
        });
        // An explicit folder argument wins over Home; mpwm passes none.
        let start = std::env::args()
            .skip(1)
            .find(|a| !a.starts_with('-'))
            .map(PathBuf::from)
            .filter(|p| vfs().is_dir(p))
            .unwrap_or_else(|| self.home.clone());
        self.tabs = vec![Tab::new(start, ViewMode::Icons)];
        self.tab = 0;
        self.enter_tab(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.menu_open && self.handle_menu_actions(cx, actions) {
            return;
        }
        if self.ui.view(cx, ids!(back_button)).finger_down(actions).is_some() {
            self.go_back(cx);
        }
        if self
            .ui
            .view(cx, ids!(forward_button))
            .finger_down(actions)
            .is_some()
        {
            self.go_forward(cx);
        }
        if self.ui.view(cx, ids!(search_button)).finger_down(actions).is_some() {
            self.set_search(cx, !self.search_visible);
        }
        if self
            .ui
            .view(cx, ids!(preview_button))
            .finger_down(actions)
            .is_some()
        {
            self.toggle_preview(cx);
        }
        if self
            .ui
            .view(cx, ids!(terminal_button))
            .finger_down(actions)
            .is_some()
        {
            self.open_terminal(cx);
        }
        if self
            .ui
            .view(cx, ids!(newfolder_button))
            .finger_down(actions)
            .is_some()
        {
            self.new_folder(cx);
        }
        if self.ui.view(cx, ids!(props_button)).finger_down(actions).is_some() {
            let open = !self.props_open;
            self.set_props(cx, open);
        }
        if self.ui.view(cx, ids!(props_close)).finger_down(actions).is_some() {
            self.set_props(cx, false);
        }
        for (id, mode) in MODE_BUTTONS {
            if self.ui.view(cx, id).finger_down(actions).is_some() {
                self.set_mode(cx, mode);
            }
        }
        if self.ui.view(cx, ids!(menu_button)).finger_down(actions).is_some() {
            let open = !self.column_menu_open;
            self.set_column_menu(cx, open);
            let thumbs = self
                .with_contents(cx, |contents, _| contents.thumbs_resident())
                .unwrap_or(0);
            let undo = self
                .journal
                .peek()
                .map(|u| format!("Cmd+Z {}", u.describe().to_lowercase()))
                .unwrap_or_else(|| "Cmd+Z undo".to_string());
            self.status(
                cx,
                &format!(
                    "Cmd+1/2/3/4 views · Cmd+T tab · Ctrl+L path · Cmd+D bookmark · F2 rename · Cmd+C/X/V · Cmd+Delete trash · {undo} · Cmd+I info · {thumbs} thumbnails cached"
                ),
            );
        }
        if self.column_menu_open {
            for (id, column) in COLUMN_ROWS {
                if self.ui.view(cx, id).finger_down(actions).is_some() {
                    self.with_contents(cx, |contents, cx| contents.toggle_column(cx, column));
                    self.refresh_column_menu(cx);
                    self.report(cx);
                    return;
                }
            }
            if self.ui.view(cx, ids!(column_menu)).finger_down(actions).is_some() {
                self.set_column_menu(cx, false);
            }
        }
        if self.ui.view(cx, ids!(quick_look)).finger_down(actions).is_some() {
            self.close_preview(cx);
        }

        // ---- tabs
        for (index, id) in TAB_IDS.iter().enumerate() {
            if index >= self.tabs.len() {
                continue;
            }
            let item = self.ui.view(cx, *id);
            let on_button = item.view(cx, ids!(tab_close)).finger_down(actions).is_some();
            let row_press = item.finger_down(actions);
            if !on_button && row_press.is_none() {
                continue;
            }
            let on_close = on_button
                || row_press
                    .map(|press| self.pressed_on(cx, *id, ids!(tab_close), press.abs))
                    .unwrap_or(false);
            self.tab = index;
            if on_close {
                self.close_tab(cx);
            } else {
                self.enter_tab(cx);
            }
            return;
        }

        // ---- progress row
        if self
            .ui
            .view(cx, ids!(progress_cancel))
            .finger_down(actions)
            .is_some()
        {
            if let (Some(engine), Some(id)) = (self.ops.as_ref(), self.active_op) {
                engine.cancel(id);
                self.status(cx, "Stopping…");
            }
        }

        // ---- batch dialog
        if self.batch_open {
            if self.ui.view(cx, ids!(batch_cancel)).finger_down(actions).is_some() {
                return self.close_batch(cx);
            }
            if self.ui.view(cx, ids!(batch_apply)).finger_down(actions).is_some() {
                return self.apply_batch(cx);
            }
            let mut changed = false;
            for id in [ids!(batch_find), ids!(batch_replace), ids!(batch_pattern)] {
                let field = self.ui.view(cx, id).text_input(cx, ids!(field_input));
                if field.changed(actions).is_some() {
                    changed = true;
                }
                if field.returned(actions).is_some() {
                    return self.apply_batch(cx);
                }
            }
            if changed {
                self.refresh_batch_preview(cx);
            }
        }

        // ---- sidebar places
        for (id, name) in PLACES {
            if self.ui.view(cx, id).finger_down(actions).is_some() {
                let path = self.place_path(name);
                self.navigate(cx, path, true);
                break;
            }
        }

        // ---- sidebar bookmarks
        let marks: Vec<PathBuf> = self.bookmarks.list().to_vec();
        for (index, id) in BOOKMARK_IDS.iter().enumerate() {
            let Some(path) = marks.get(index) else {
                break;
            };
            let item = self.ui.view(cx, *id);
            if item.finger_hover_in(actions).is_some() && self.hovered_bookmark != Some(index) {
                self.hovered_bookmark = Some(index);
                self.refresh_bookmarks(cx);
            }
            if let Some(left) = item.finger_hover_out(actions) {
                // Moving onto the remove button *is* a hover-out of the row —
                // hover belongs to one area at a time. Clearing on that would
                // hide the button the moment it was aimed at, so the row keeps
                // its hover until the pointer leaves the row itself.
                let row = self.ui.view(cx, *id).area().rect(cx);
                if !row.contains(left.abs) && self.hovered_bookmark == Some(index) {
                    self.hovered_bookmark = None;
                    self.refresh_bookmarks(cx);
                }
            }
            // Whichever of the two saw the press: the button when it is
            // visible and takes the capture, the row otherwise — and then its
            // position says which was meant.
            let on_button = item.view(cx, ids!(bm_remove)).finger_down(actions).is_some();
            let row_press = item.finger_down(actions);
            if !on_button && row_press.is_none() {
                continue;
            }
            let path = path.clone();
            let on_remove = on_button
                || (self.hovered_bookmark == Some(index)
                    && row_press
                        .map(|press| self.pressed_on(cx, *id, ids!(bm_remove), press.abs))
                        .unwrap_or(false));
            if on_remove {
                let name = display_name(&path);
                self.bookmarks.remove(&path);
                self.hovered_bookmark = None;
                self.refresh_bookmarks(cx);
                self.status(cx, &format!("Removed the {name} bookmark"));
            } else {
                self.navigate(cx, path, true);
            }
            return;
        }

        // ---- path bar: a crumb navigates, the empty space opens the editor
        let widget = self.ui.widget(cx, ids!(breadcrumbs));
        let crumb = widget
            .borrow::<Breadcrumbs>()
            .and_then(|breadcrumbs| breadcrumbs.clicked_path(cx, actions));
        if let Some(path) = crumb {
            self.navigate(cx, path, true);
        } else if !self.search_visible
            && self.ui.view(cx, ids!(crumb_box)).finger_down(actions).is_some()
        {
            self.set_path_edit(cx, true);
        }
        let path_field = self.ui.text_input(cx, ids!(path_edit));
        if let Some((text, _)) = path_field.returned(actions) {
            self.commit_path_edit(cx, &text);
        }
        if path_field.escaped(actions) && self.path_edit_open {
            self.set_path_edit(cx, false);
        }

        if let Some(filter) = self.ui.text_input(cx, ids!(search_input)).changed(actions) {
            self.with_contents(cx, |contents, cx| contents.set_filter(cx, filter));
            let shown = self.with_contents(cx, |contents, _| (contents.len(), contents.total()));
            if let Some((shown, total)) = shown {
                self.ui
                    .label(cx, ids!(item_count))
                    .set_text(cx, &format!("{} of {} items", shown, total));
            }
        }

        let body = self
            .with_contents(cx, |contents, cx| contents.handle_actions(cx, actions))
            .unwrap_or_default();
        for action in body {
            self.handle_contents_action(cx, action);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        // The WM's theme, first into the stock widgets and then into `mod.mpf`
        // for our own chrome — both before anything reads a color.
        mp_theme::apply(vm);
        Palette::shared().publish(vm);
        crate::theme::script_mod(vm);
        crate::thumbs::script_mod(vm);
        crate::treemap_view::script_mod(vm);
        crate::contents::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        if let Event::Signal = event {
            self.drain_directory_results(cx);
            self.drain_ops(cx);
            self.drain_sizes(cx);
            self.with_contents(cx, |contents, cx| {
                contents.drain_thumbs(cx);
                contents.treemap(cx).drain(cx)
            });
            if self.tabs.get(self.tab).map(|t| t.mode) == Some(ViewMode::Treemap) {
                self.report(cx);
            }
            self.preview.poll();
        }
        if let Event::Custom(json) = event {
            if let Some(wm) = mp_wm_api::WmEvent::parse(json) {
                self.handle_wm_event(cx, &wm);
            }
        }
        if self.focus_next.is_event(event).is_some() {
            self.apply_focus(cx);
        }
        if let Event::KeyDown(key) = event {
            self.handle_key(cx, key);
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
