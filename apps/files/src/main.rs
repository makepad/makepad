//! files — the file browser of the Makepad desktop.
//!
//! A GNOME-Files-shaped browser: tabs, a places-and-bookmarks sidebar, an
//! editable breadcrumb path bar, and four views over one folder (icons with
//! real thumbnails, a sortable DataGrid list with expandable folders, a
//! compact list, and a treemap of where the bytes actually are). Space quick-
//! looks the selection the way macOS does; inside wm the compositor hosts
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
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
};

#[cfg(not(target_arch = "wasm32"))]
use std::process::Command;

mod bookmarks;
#[cfg(feature = "chat")]
mod chat_agent;
#[cfg(feature = "chat")]
mod chat_panel;
mod ai_service;
mod chat_tools;
mod contents;
mod demo;
mod menu;
mod model;
mod ops;
mod preview;
mod rename;
mod sizecache;
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
    ops::{Journal, OpKind, OpRequest, OpUpdate, Ops, Undo},
    preview::{Preview, PreviewHost},
    rename::BatchMode,
    theme::Palette,
    treemap_view::MapProjection,
    vfs::vfs,
};

#[cfg(feature = "chat")]
use crate::{
    chat_agent::{ChatAgent, ChatEvent},
    chat_panel::{ChatState, ChatVoice},
    chat_tools::{ToolJob, ToolRunner},
};
use crate::ai_service::{ServiceReply, ServiceRunner};
use makepad_ai_services::port::{AiServicePort, PortEvent};

#[cfg(not(feature = "chat"))]
mod no_chat {
    use makepad_widgets::*;

    script_mod! {
        use mod.prelude.widgets.*
        mod.widgets.MpfChatPanel = View{visible: false width: 0 height: 0}
    }
}

app_main!(App, font_set: International);

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

    /** One row of the filter popup's legend: swatch, kind, live bytes.
     * Clicking it IS the filter toggle for that kind. */
    let LegendRow = SolidView{
        width: Fill
        height: 22
        flow: Right
        spacing: 8
        padding: Inset{left: 8 right: 8}
        align: Align{y: 0.5}
        cursor: MouseCursor.Hand
        draw_bg +: {color: #00000000}
        lg_swatch := SolidView{
            width: 10
            height: 10
            draw_bg +: {color: #x565f89}
        }
        lg_name := Label{
            width: Fill
            draw_text +: {
                color: mod.mpf.fg
                text_style: theme.font_regular{font_size: 9.5}
            }
        }
        lg_bytes := Label{
            draw_text +: {
                color: mod.mpf.fg_dim
                text_style: theme.font_regular{font_size: 9.5}
            }
        }
    }

    /** One "modified within" choice. The box is the text plus the same
     * padding on every side, so the highlight is centred by construction —
     * a fixed height had the label riding low in it. */
    let AgeChip = SolidView{
        width: Fit
        height: Fit
        padding: Inset{left: 5 right: 5 top: 3 bottom: 3}
        cursor: MouseCursor.Hand
        draw_bg +: {color: #00000000}
        chip_label := Label{
            draw_text +: {
                color: mod.mpf.fg_dim
                text_style: theme.font_regular{font_size: 9.0}
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
                            chat_button := ToolButton{
                                visible: #(cfg!(feature = "chat"))
                                Icon{
                                    icon_walk: Walk{width: 15 height: 15}
                                    draw_icon +: {
                                        svg: crate_resource("self://resources/icons/chat.svg")
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

                                // The map's own tool strip. It only exists in
                                // the Treemap view, and everything on it acts
                                // on the rectangle that is picked — which is
                                // what a right-click used to be for.
                                map_tools := SolidView{
                                    visible: false
                                    width: Fill
                                    height: 30
                                    flow: Right
                                    spacing: 2
                                    padding: Inset{left: 16 right: 16}
                                    align: Align{y: 0.5}
                                    draw_bg +: {color: mod.mpf.bg_dark}
                                    // The render-mode switch: one block view,
                                    // three ways of looking at it.
                                    proj_flat := ToolButton{
                                        Icon{
                                            icon_walk: Walk{width: 15 height: 15}
                                            draw_icon +: {
                                                svg: crate_resource("self://resources/icons/treemap.svg")
                                                color: mod.mpf.fg
                                            }
                                        }
                                    }
                                    proj_ortho := ToolButton{
                                        Icon{
                                            icon_walk: Walk{width: 15 height: 15}
                                            draw_icon +: {
                                                svg: crate_resource("self://resources/icons/treemap25.svg")
                                                color: mod.mpf.fg
                                            }
                                        }
                                    }
                                    proj_persp := ToolButton{
                                        Icon{
                                            icon_walk: Walk{width: 15 height: 15}
                                            draw_icon +: {
                                                svg: crate_resource("self://resources/icons/treemap3d.svg")
                                                color: mod.mpf.fg
                                            }
                                        }
                                    }
                                    View{width: 10 height: 1}
                                    map_rescan := ToolButton{
                                        map_rescan_icon := Icon{
                                            icon_walk: Walk{width: 15 height: 15}
                                            draw_icon +: {
                                                svg: crate_resource("self://resources/icons/reload.svg")
                                                color: mod.mpf.fg
                                            }
                                        }
                                    }
                                    View{width: 10 height: 1}
                                    map_trash := ToolButton{
                                        map_trash_icon := Icon{
                                            icon_walk: Walk{width: 15 height: 15}
                                            draw_icon +: {
                                                svg: crate_resource("self://resources/icons/trash.svg")
                                                color: mod.mpf.muted
                                            }
                                        }
                                    }
                                    map_erase := ToolButton{
                                        map_erase_icon := Icon{
                                            icon_walk: Walk{width: 15 height: 15}
                                            draw_icon +: {
                                                svg: crate_resource("self://resources/icons/delete-forever.svg")
                                                color: mod.mpf.muted
                                            }
                                        }
                                    }
                                    View{width: 10 height: 1}
                                    map_filter := ToolButton{
                                        map_filter_icon := Icon{
                                            icon_walk: Walk{width: 15 height: 15}
                                            draw_icon +: {
                                                svg: crate_resource("self://resources/icons/filter.svg")
                                                color: mod.mpf.fg
                                            }
                                        }
                                    }
                                    map_tools_hint := Label{
                                        width: Fill
                                        max_lines: 1
                                        margin: Inset{left: 8}
                                        text_overflow: TextOverflow.Ellipsis
                                        text: "Click a rectangle to pick it"
                                        draw_text +: {
                                            color: mod.mpf.fg_dim
                                            text_style: theme.font_regular{font_size: 8.5}
                                        }
                                    }
                                    map_scan_all := CheckBox{
                                        text: "ignore system"
                                    }
                                }
                                map_row := View{
                                    width: Fill
                                    height: Fill
                                    flow: Right
                                    contents := mod.widgets.FileContents{}
                                    // The filter, docked: everything in it
                                    // applies live, and the map tweens right
                                    // beside it while you fiddle.
                                    map_side := SolidView{
                                        visible: false
                                        width: 258
                                        height: Fill
                                        draw_bg +: {color: mod.mpf.bg_dark}
                                        ScrollYView{
                                            width: Fill
                                            height: Fill
                                            flow: Down
                                            spacing: 6
                                            padding: Inset{left: 10 right: 10 top: 10 bottom: 10}
                                            Label{
                                                text: "FILTER"
                                                draw_text +: {
                                                    color: mod.mpf.fg_dim
                                                    text_style: theme.font_bold{font_size: 8.0}
                                                }
                                            }
                                            filter_query := MpfInput{
                                                width: Fill
                                                height: 26
                                                empty_text: "name, .ext, >100mb, <7d"
                                            }
                                            View{
                                                width: Fill
                                                height: Fit
                                                flow: Right
                                                spacing: 8
                                                align: Align{y: 0.5}
                                                filter_size_label := Label{
                                                    width: 96
                                                    text: "any size"
                                                    draw_text +: {
                                                        color: mod.mpf.fg_dim
                                                        text_style: theme.font_regular{font_size: 9.0}
                                                    }
                                                }
                                                filter_size := Slider{
                                                    width: Fill
                                                    height: 18
                                                    text: ""
                                                }
                                            }
                                            filter_age_row := View{
                                                width: Fill
                                                height: Fit
                                                flow: Right
                                                spacing: 2
                                                align: Align{y: 0.5}
                                                filter_age_hint := Label{
                                                    margin: Inset{right: 4}
                                                    text: "new:"
                                                    draw_text +: {
                                                        color: mod.mpf.fg_dim
                                                        text_style: theme.font_regular{font_size: 9.0}
                                                    }
                                                }
                                                filter_age0 := AgeChip{chip_label +: {text: "any"}}
                                                filter_age1 := AgeChip{chip_label +: {text: "1d"}}
                                                filter_age2 := AgeChip{chip_label +: {text: "3d"}}
                                                filter_age3 := AgeChip{chip_label +: {text: "1w"}}
                                                filter_age4 := AgeChip{chip_label +: {text: "1mo"}}
                                                filter_age5 := AgeChip{chip_label +: {text: "1y"}}
                                            }
                                            Hr{}
                                            filter_kind0 := LegendRow{}
                                            filter_kind1 := LegendRow{}
                                            filter_kind2 := LegendRow{}
                                            filter_kind3 := LegendRow{}
                                            filter_kind4 := LegendRow{}
                                            filter_kind5 := LegendRow{}
                                            filter_kind6 := LegendRow{}
                                            filter_clear := View{
                                                width: Fill
                                                height: 20
                                                align: Align{x: 1.0 y: 0.5}
                                                cursor: MouseCursor.Hand
                                                clear_label := Label{
                                                    text: "clear all"
                                                    draw_text +: {
                                                        color: mod.mpf.accent
                                                        text_style: theme.font_regular{font_size: 9.0}
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

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

                            chat_panel := mod.widgets.MpfChatPanel{}
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
    #[cfg(feature = "chat")]
    Chat,
    Filter,
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

/// The projection switch on the map's own strip: how the block view renders,
/// not which view is open.
const PROJ_BUTTONS: [(&[LiveId], MapProjection); 3] = [
    (ids!(proj_flat), MapProjection::Flat),
    (ids!(proj_ortho), MapProjection::Ortho),
    (ids!(proj_persp), MapProjection::Persp),
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

/// The warm-pool dormancy state machine (see `makepad_wm_api::warm_start` /
/// `WmEvent::Adopted`). wm pre-spawns hidden warm instances of this app
/// (`MAKEPAD_WM_WARM_START=1`); a cached file browser must not scan a directory or
/// decode thumbnails for a window nobody is looking at. A warm instance
/// starts `Dormant` — no initial directory scan — and wakes exactly once:
/// either wm adopts it into a real tile (`WmEvent::Adopted` on the studio
/// `Custom` channel), or, defensively, a human touches the window directly
/// (a key or pointer/touch event, in case an `Adopted` message is ever
/// lost). A non-warm instance is never dormant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dormancy {
    /// Not a warm-pool instance: the initial scan happens immediately.
    #[default]
    Active,
    /// A warm-pool instance, still idling.
    Dormant,
    /// A warm-pool instance that has woken up.
    Woken,
}

impl Dormancy {
    /// `warm` is `makepad_wm_api::warm_start()`, read once at startup.
    pub fn start(warm: bool) -> Self {
        if warm { Dormancy::Dormant } else { Dormancy::Active }
    }

    pub fn is_dormant(&self) -> bool {
        *self == Dormancy::Dormant
    }

    /// Transition `Dormant` -> `Woken`. Returns `true` the one time this
    /// actually wakes it (the caller should run the deferred scan then);
    /// `false` when it was already active or already woken, so `Adopted`
    /// arriving after an input wake (or twice) never rescans.
    pub fn wake(&mut self) -> bool {
        if *self == Dormancy::Dormant {
            *self = Dormancy::Woken;
            true
        } else {
            false
        }
    }
}

/// A raw input event a human — not the WM protocol — could only have sent:
/// the defensive wake path for a lost `WmEvent::Adopted`.
fn is_wake_input(event: &Event) -> bool {
    matches!(
        event,
        Event::KeyDown(_) | Event::MouseDown(_) | Event::TouchUpdate(_)
    )
}

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
    /// The filter sidebar, docked to the right of the map. The name kept its
    /// popup days; the state is the same choice.
    #[rust]
    filter_popup_open: bool,
    /// How the block view renders — flat, extruded, perspective. A property
    /// of the view, not a view of its own; saved across launches.
    #[rust]
    projection: MapProjection,
    /// The "modified within" choice: an index into [`AGE_MINUTES`].
    #[rust]
    filter_age: usize,
    /// Which legend kinds are toggled into the filter.
    #[rust]
    filter_kinds: [bool; 7],
    /// Which kind class each legend row currently shows (rows are sorted by
    /// bytes, so the mapping moves).
    #[rust]
    legend_rows: [usize; 7],
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
    /// What each running job will mean for the size map once it lands. The
    /// map is expensive to build and cheap to correct, so every operation the
    /// app performs itself is folded straight into it — a scan of a full home
    /// directory is minutes, and moving one file to the Trash should not cost
    /// them.
    #[rust]
    map_jobs: Vec<MapJob>,
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
    /// Warm-pool dormancy — see `Dormancy`.
    #[rust]
    dormancy: Dormancy,

    // ---------------------------------------------------------------- chat
    /// The ask-about-these-files panel. Everything below it stays `None` until
    /// the panel is opened for the first time: a file browser must not load
    /// nine billion parameters for a panel nobody asked for.
    #[cfg(feature = "chat")]
    #[rust]
    chat_open: bool,
    #[cfg(feature = "chat")]
    #[rust]
    chat: ChatState,
    #[cfg(feature = "chat")]
    #[rust]
    agent: Option<ChatAgent>,
    #[cfg(feature = "chat")]
    #[rust]
    tool_runner: Option<ToolRunner>,
    /// True between sending a question and the answer being finished.
    #[cfg(feature = "chat")]
    #[rust]
    chat_busy: bool,
    #[cfg(feature = "chat")]
    #[rust]
    chat_ready: bool,
    /// How many tool results the model is still owed for this turn, and the
    /// ones that have come back so far — they go over in call order, together.
    #[cfg(feature = "chat")]
    #[rust]
    chat_awaiting_tools: usize,
    #[cfg(feature = "chat")]
    #[rust]
    chat_tool_replies: Vec<ToolReply>,
    /// Tool rounds spent on the current question, so a model that decides to
    /// keep looking forever is stopped rather than left running.
    #[cfg(feature = "chat")]
    #[rust]
    chat_tool_rounds: usize,
    /// The status line under the panel header.
    #[cfg(feature = "chat")]
    #[rust]
    chat_status: String,
    /// The last "about:" chip and map-strip hint that were pushed into the UI,
    /// so the per-signal refresh only touches a widget when something changed.
    #[cfg(feature = "chat")]
    #[rust]
    chat_about: String,
    #[rust]
    map_tools_note: String,

    // ------------------------------------------------------------- AI bus
    /// The app's service on the desktop's AI bus: open while the window
    /// manager hosts this process, `None` standalone (see ai_service.rs).
    #[rust]
    ai_port: Option<AiServicePort>,
    /// The bus's tool worker, made on the first call.
    #[rust]
    ai_runner: Option<ServiceRunner>,
    /// The context line last sent over the bus, so a selection that did not
    /// change sends nothing.
    #[rust]
    ai_context: String,
}

/// One finished tool call, waiting for its turn-mates.
#[cfg(feature = "chat")]
pub struct ToolReply {
    text: String,
    is_error: bool,
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
            #[cfg(feature = "chat")]
            FocusTarget::Chat => self.ui.text_input(cx, ids!(chat_input)),
            FocusTarget::Filter => self.ui.text_input(cx, ids!(filter_query)),
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
        if self.tabs[self.tab].mode.is_treemap() {
            let map = self.with_contents(cx, |contents, cx| contents.treemap(cx));
            if let Some(map) = map {
                map.set_root(cx, &path);
            }
        }
        let dir = path.clone();
        if vfs().is_instant() {
            let result = vfs().read_dir(&path, show_hidden);
            let _ = sender.send(DirectoryResult { dir, request_id, parent: None, result });
            self.drain_directory_results(cx);
            return;
        }
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
        if vfs().is_instant() {
            let result = vfs().read_dir(&folder, show_hidden);
            let _ = sender.send(DirectoryResult { dir, request_id, parent: Some(folder), result });
            self.drain_directory_results(cx);
            return;
        }
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
        // Whatever changed, the chat's "about:" chip and the map strip are
        // about the same selection this line is — so they follow it here.
        self.refresh_chat(cx);
        let mode = self.tabs[self.tab].mode;
        if mode.is_treemap() {
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
            "network" if vfs().is_demo() => self.home.join("Network"),
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
        let projection = self.projection;
        self.with_contents(cx, |contents, cx| {
            contents.set_mode(cx, mode);
            let map = contents.treemap(cx);
            // Scanning a tree is expensive: it only runs while the map is the
            // thing on screen.
            if mode.is_treemap() {
                if map.root() != dir {
                    map.set_root(cx, &dir);
                }
                map.set_projection(cx, projection);
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
        self.style_projection_buttons(cx);
        // The map's tool strip and the filter sidebar belong to the map. The
        // pick it acts on lives in the treemap widget and survives this, so
        // coming back to the map finds the same rectangle still ringed.
        self.ui
            .widget(cx, ids!(map_tools))
            .set_visible(cx, mode.is_treemap());
        self.ui
            .widget(cx, ids!(map_side))
            .set_visible(cx, mode.is_treemap() && self.filter_popup_open);
        if mode.is_treemap() && self.filter_popup_open {
            // Entering the map with the sidebar already open (a pref, or a
            // mode round-trip): the legend fills now, not on the next toggle.
            self.refresh_filter_popup(cx);
        }
        self.map_tools_note.clear();
        self.refresh_chat(cx);
    }

    /// Choose how the block view renders, remember it, and light the right
    /// button. Never changes which view is open.
    fn set_projection_choice(&mut self, cx: &mut Cx, projection: MapProjection) {
        self.projection = projection;
        model::pref_set(
            "projection",
            match projection {
                MapProjection::Flat => "flat",
                MapProjection::Ortho => "ortho",
                MapProjection::Persp => "persp",
            },
        );
        self.with_contents(cx, |contents, cx| {
            contents.treemap(cx).set_projection(cx, projection);
        });
        self.style_projection_buttons(cx);
        self.report(cx);
        self.ui.redraw(cx);
    }

    fn style_projection_buttons(&mut self, cx: &mut Cx) {
        for (id, projection) in PROJ_BUTTONS {
            self.ui
                .widget(cx, id)
                .widget(cx, ids!(btn_sel))
                .set_visible(cx, projection == self.projection);
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
        if !makepad_wm_api::hosted(cx) {
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

    // --------------------------------------------------------------- AI bus

    /// Open the service toward the window manager. Nothing standalone (the
    /// port says no); nothing twice.
    fn open_ai_port(&mut self, cx: &mut Cx) {
        if self.ai_port.is_some() {
            return;
        }
        self.ai_port = AiServicePort::hosted(cx, chat_tools::service_manifest());
        if self.ai_port.is_some() {
            log!("files: AI service opened toward the window manager");
        }
    }

    fn on_ai_port_events(&mut self, cx: &mut Cx, events: Vec<PortEvent>) {
        for event in events {
            match event {
                PortEvent::Registered(endpoint) => {
                    log!("files: AI service registered as {}", endpoint.as_str());
                    // A (re)registration starts the context afresh.
                    self.ai_context.clear();
                    self.refresh_ai_context(cx);
                }
                PortEvent::Call(call) => {
                    let (cwd, home) = (self.current_dir(), self.home.clone());
                    self.ai_runner
                        .get_or_insert_with(ServiceRunner::new)
                        .submit(&call, cwd, home);
                }
                PortEvent::Cancel { call_id } => {
                    if let Some(runner) = self.ai_runner.as_mut() {
                        runner.cancel(&call_id);
                    }
                }
                PortEvent::ChatOpen { open } => {
                    // The desktop's pane is the chat now: the app's own panel
                    // steps aside (Cmd+K brings it back on purpose).
                    #[cfg(feature = "chat")]
                    if open && self.chat_open {
                        self.toggle_chat(cx);
                    }
                    #[cfg(not(feature = "chat"))]
                    let _ = open;
                }
            }
        }
    }

    /// The worker's answers and progress go back over the port.
    fn drain_ai_replies(&mut self, _cx: &mut Cx) {
        let Some(runner) = self.ai_runner.as_mut() else {
            return;
        };
        let replies = runner.drain();
        let Some(port) = self.ai_port.as_ref() else {
            return;
        };
        for reply in replies {
            match reply {
                ServiceReply::Result(result) => port.reply(result),
                ServiceReply::Progress {
                    call_id,
                    note,
                    permille,
                } => port.progress(&call_id, &note, permille),
            }
        }
    }

    /// What the assistant is told about where the person is — the folder,
    /// the view and the selection — whenever that changes. This is how
    /// "my downloads" means the folder on screen.
    fn refresh_ai_context(&mut self, cx: &mut Cx) {
        if self.ai_port.is_none() {
            return;
        }
        let text = self.ai_context_line(cx);
        if text == self.ai_context {
            return;
        }
        self.ai_context = text.clone();
        if let Some(port) = self.ai_port.as_ref() {
            port.set_context(&text);
        }
    }

    fn ai_context_line(&mut self, cx: &mut Cx) -> String {
        let Some(tab) = self.tabs.get(self.tab) else {
            return String::new();
        };
        let mode = tab.mode;
        let dir = self.current_dir();
        let mut out = format!(
            "The person is looking at {} in the {} view.",
            chat_tools::short(&dir, &self.home),
            mode.label(),
        );
        let selected = self
            .with_contents(cx, |contents, _| contents.selected_entries())
            .unwrap_or_default();
        if selected.is_empty() {
            out.push_str(" Nothing is selected.");
        } else {
            out.push_str(&format!(" Selected ({}):", selected.len()));
            for entry in selected.iter().take(12) {
                out.push_str(&format!(
                    " {} ({}, {});",
                    entry.name,
                    entry.kind_text(),
                    entry.size_text()
                ));
            }
            if selected.len() > 12 {
                out.push_str(&format!(" …and {} more", selected.len() - 12));
            }
        }
        out
    }

    /// The window manager's side of the conversation.
    fn handle_wm_event(&mut self, cx: &mut Cx, event: &makepad_wm_api::WmEvent) {
        if matches!(event, makepad_wm_api::WmEvent::Adopted) {
            self.wake(cx);
            // Adopted into a real tile: now it is a running Files.
            self.open_ai_port(cx);
        }
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
        let paths = self.target_paths(cx);
        if paths.is_empty() {
            self.status(cx, "Nothing selected to duplicate");
            return;
        }
        self.submit(cx, OpKind::Copy, paths, None);
    }

    /// Measure the folder again and replace the map that was read back from
    /// the cache. The one thing that makes a remembered map safe: it is never
    /// more than a keystroke from being made true.
    fn rescan_map(&mut self, cx: &mut Cx) {
        if !self.tabs[self.tab].mode.is_treemap() {
            self.status(cx, "Rescanning is for the map — Cmd+4 shows it");
            return;
        }
        self.with_contents(cx, |contents, cx| contents.treemap(cx).rescan(cx));
        self.report(cx);
    }

    /// Show the entry on the map. The map is of the folder we are in, so this
    /// is a view change plus a highlight — not a search.
    fn reveal_in_treemap(&mut self, cx: &mut Cx, entry: Option<FileEntry>) {
        if !self.tabs[self.tab].mode.is_treemap() {
            self.set_mode(cx, ViewMode::Treemap);
        }
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
        let paths = self.target_paths(cx);
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
        let now = vfs::now_secs();
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
                "files"
            } else {
                makepad_wm_api::viewer_for(&path)
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
        if vfs().is_instant() {
            let bytes = vfs().total_bytes(&path, &cancel);
            let _ = sender.send(SizeResult { path, bytes });
            self.drain_sizes(cx);
            return;
        }
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
        // Remembered now, applied when the job reports back: the size map can
        // be corrected by arithmetic instead of another walk of the disk, but
        // only if it knows what went where, and `OpUpdate::Done` says only
        // where things landed.
        self.remember_for_map(id, MapEffect::of(kind), sources.clone());
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
                    self.map_absorb(cx, id, &touched);
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
                    // Nothing happened, so the map is still right.
                    self.map_jobs.retain(|job| job.id != id);
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
        let paths = self.target_paths(cx);
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

    /// The paths a file operation starts from.
    ///
    /// Normally that is the listing's selection. On the treemap it usually
    /// cannot be: nearly everything the map draws lives below the folder being
    /// listed, so the map's own pick is the answer instead. Telling somebody
    /// mid-cleanup that nothing is selected while a 6 GB rectangle sits
    /// outlined in front of them would be a lie.
    fn target_paths(&mut self, cx: &mut Cx) -> Vec<PathBuf> {
        let paths: Vec<PathBuf> = self
            .with_contents(cx, |contents, _| contents.selected_entries())
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.path)
            .collect();
        if !paths.is_empty() || !self.tabs[self.tab].mode.is_treemap() {
            return paths;
        }
        self.with_contents(cx, |contents, cx| contents.treemap(cx).selection())
            .flatten()
            .map(|path| vec![path])
            .unwrap_or_default()
    }

    fn trash_selection(&mut self, cx: &mut Cx) {
        let paths = self.target_paths(cx);
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
        // An undo is a move backwards or a removal, and both sides of it are
        // already known — so the map follows it without a rescan too.
        match &undo {
            Undo::Moved { pairs } => {
                let sources: Vec<PathBuf> = pairs.iter().map(|(_, to)| to.clone()).collect();
                self.remember_for_map(id, MapEffect::Move, sources);
            }
            Undo::Created { paths } => {
                self.remember_for_map(id, MapEffect::Remove, paths.clone());
            }
        }
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
                if self.tabs[self.tab].mode.is_treemap() {
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
        if vfs().is_demo() {
            self.status(cx, "Terminal is not in this demo");
            return;
        }
        let dir = self.current_dir();
        let request = makepad_wm_api::WmRequest::Launch {
            app: "terminal".to_string(),
            args: vec!["--cwd".to_string(), dir.display().to_string()],
        };
        if makepad_wm_api::send(cx, &request) {
            self.status(cx, &format!("Opening a terminal in {}", dir.display()));
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(bin) = preview::sibling_bin("terminal") else {
                self.status(cx, "terminal is not built — nothing to open a terminal with");
                return;
            };
            match Command::new(&bin).arg("--cwd").arg(&dir).spawn() {
                Ok(_) => self.status(cx, &format!("Opening a terminal in {}", dir.display())),
                Err(error) => self.status(cx, &format!("Could not start terminal: {error}")),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.status(cx, "Terminal is not in this demo");
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
        // The listing's selection is what "this" means outside the map, so the
        // ask panel's chip follows it here the same way it follows the pick.
        self.refresh_chat(cx);
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
                makepad_wm_api::viewer_for(&entry.path),
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
            // The caret in the filter's query field first: Escape clears what
            // is typed there, and only an already-empty field lets Escape
            // mean anything bigger. The sidebar itself is the funnel's to
            // close, never Escape's — a surprise-closing panel loses work.
            if self.filter_popup_open && self.filter_is_typing(cx) {
                let field = self.ui.text_input(cx, ids!(filter_query));
                if !field.text().is_empty() {
                    field.set_text(cx, "");
                    self.rebuild_filter(cx);
                    return;
                }
            }
            if self.menu_open {
                return self.close_menu(cx);
            }
            // Only while the caret is actually in the ask field: Escape on the
            // map still means "zoom back out", panel or no panel.
            #[cfg(feature = "chat")]
            if self.chat_open && self.chat_is_typing(cx) {
                return self.toggle_chat(cx);
            }
            // A zoomed treemap is one of the things Escape is on top of: it
            // steps back out one folder before Escape means anything else.
            if self.tabs[self.tab].mode.is_treemap()
                && self
                    .with_contents(cx, |contents, cx| contents.treemap(cx).zoom_out(cx))
                    .unwrap_or(false)
            {
                return self.report(cx);
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
        // so what is *open* decides whether a key is text or navigation. The
        // chat's field is the exception: the panel stays open while the user
        // reads, so it is the keyboard that says whether they are typing in it.
        // The filter's query field counts too: without it, typing a word into
        // the filter let Backspace fall through to "go up" — which navigated
        // away and started a whole new scan mid-keystroke — and q/e spun the
        // camera under the caret.
        let editing = self.batch_open
            || self.path_edit_open
            || self.search_visible
            || self.chat_is_typing(cx)
            || self.filter_is_typing(cx)
            || self
                .with_contents(cx, |contents, _| contents.is_renaming())
                .unwrap_or(false);

        if command {
            match event.key_code {
                #[cfg(feature = "chat")]
                KeyCode::KeyK => return self.toggle_chat(cx),
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
                KeyCode::KeyR if !editing => return self.rescan_map(cx),
                KeyCode::Key1 => return self.set_mode(cx, ViewMode::Icons),
                KeyCode::Key2 => return self.set_mode(cx, ViewMode::List),
                KeyCode::Key3 => return self.set_mode(cx, ViewMode::Compact),
                // The block view and its three renderings: Cmd+4 the flat
                // map, Cmd+5 the extrusion, Cmd+6 the perspective — each
                // enters the view if it is not already open.
                KeyCode::Key4 => {
                    self.set_projection_choice(cx, MapProjection::Flat);
                    return self.set_mode(cx, ViewMode::Treemap);
                }
                KeyCode::Key5 => {
                    self.set_projection_choice(cx, MapProjection::Ortho);
                    return self.set_mode(cx, ViewMode::Treemap);
                }
                KeyCode::Key6 => {
                    self.set_projection_choice(cx, MapProjection::Persp);
                    return self.set_mode(cx, ViewMode::Treemap);
                }
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
        if event.key_code == KeyCode::F5 && !editing {
            return self.rescan_map(cx);
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
        // On the map, Enter zooms into the picked folder and Backspace steps
        // back out of one — the same pair the list view uses for open and go
        // up, meaning the same two things one level in.
        if self.tabs[self.tab].mode.is_treemap() {
            match event.key_code {
                KeyCode::ReturnKey | KeyCode::NumpadEnter => {
                    if self
                        .with_contents(cx, |contents, cx| {
                            contents.treemap(cx).zoom_into_selection(cx)
                        })
                        .unwrap_or(false)
                    {
                        return self.report(cx);
                    }
                }
                KeyCode::Backspace => {
                    if self
                        .with_contents(cx, |contents, cx| contents.treemap(cx).zoom_out(cx))
                        .unwrap_or(false)
                    {
                        return self.report(cx);
                    }
                }
                // Q and E step the orbit, the keyboard's version of the
                // left-drag. A no-op on the flat map.
                KeyCode::KeyQ => {
                    self.with_contents(cx, |contents, cx| {
                        contents.treemap(cx).orbit_by(cx, -0.26, 0.0);
                    });
                    return;
                }
                KeyCode::KeyE => {
                    self.with_contents(cx, |contents, cx| {
                        contents.treemap(cx).orbit_by(cx, 0.26, 0.0);
                    });
                    return;
                }
                _ => {}
            }
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

    /// Note what a job will do to the size map, so its completion can be
    /// folded in rather than triggering a rescan. Bounded: a job that never
    /// reports back must not leave a record here forever.
    fn remember_for_map(&mut self, id: u64, effect: MapEffect, sources: Vec<PathBuf>) {
        if matches!(effect, MapEffect::Nothing) || sources.is_empty() {
            return;
        }
        if self.map_jobs.len() >= 32 {
            self.map_jobs.remove(0);
        }
        self.map_jobs.push(MapJob {
            id,
            effect,
            sources,
        });
    }

    /// Correct the size map for a job that just finished. `touched` is where
    /// things ended up, in the same order as the sources that produced them.
    fn map_absorb(&mut self, cx: &mut Cx, id: u64, touched: &[PathBuf]) {
        let Some(index) = self.map_jobs.iter().position(|job| job.id == id) else {
            return;
        };
        let job = self.map_jobs.remove(index);
        let map = self.with_contents(cx, |contents, cx| contents.treemap(cx));
        let Some(map) = map else { return };
        match job.effect {
            MapEffect::Nothing => {}
            MapEffect::Remove => {
                let moves: Vec<(PathBuf, Option<PathBuf>)> =
                    job.sources.into_iter().map(|from| (from, None)).collect();
                map.absorb_moves(cx, &moves);
            }
            MapEffect::Move => {
                // A job that reported fewer destinations than sources did not
                // move all of them; the ones it cannot account for are treated
                // as gone from where they were, which is the one thing that is
                // certainly true.
                let moves: Vec<(PathBuf, Option<PathBuf>)> = job
                    .sources
                    .into_iter()
                    .enumerate()
                    .map(|(i, from)| (from, touched.get(i).cloned()))
                    .collect();
                map.absorb_moves(cx, &moves);
            }
            MapEffect::Copy => {
                let copies: Vec<(PathBuf, PathBuf)> = job
                    .sources
                    .into_iter()
                    .zip(touched.iter().cloned())
                    .collect();
                map.absorb_copies(cx, &copies);
            }
        }
    }

    fn handle_contents_action(&mut self, cx: &mut Cx, action: FileContentsAction) {
        match action {
            FileContentsAction::Open(entry) => self.open_entry(cx, entry),
            // On the map the status line belongs to the map: it says what is
            // on screen and what was picked, which is more than one entry's
            // description and never goes stale behind it.
            FileContentsAction::Selected(entry) => {
                if self.tabs[self.tab].mode.is_treemap() {
                    self.report(cx)
                } else {
                    self.describe(cx, &entry)
                }
            }
            FileContentsAction::Sorted | FileContentsAction::Restated => self.report(cx),
            FileContentsAction::MapFilterCleared => self.reset_filter_controls(cx),
            FileContentsAction::Renamed(path, name) => self.commit_rename(cx, path, name),
            FileContentsAction::RenameCancelled => self.report(cx),
            FileContentsAction::Dropped(paths, at) => self.handle_drop(cx, paths, at),
            FileContentsAction::NeedChildren(folder) => self.request_children(cx, folder),
            FileContentsAction::Context { at, entry } => self.open_menu(cx, at, entry),
        }
    }
}

/// What a finished operation does to the size map.
#[derive(Clone, Copy, PartialEq)]
enum MapEffect {
    /// The sources stop existing anywhere the map can see.
    Remove,
    /// The sources end up somewhere else, which may or may not be on the map.
    Move,
    /// The sources stay and are duplicated.
    Copy,
    /// Nothing worth correcting: a new empty folder is no bytes.
    Nothing,
}

impl MapEffect {
    fn of(kind: OpKind) -> MapEffect {
        match kind {
            OpKind::Delete => MapEffect::Remove,
            OpKind::Trash | OpKind::Move | OpKind::Rename => MapEffect::Move,
            OpKind::Copy => MapEffect::Copy,
            OpKind::NewFolder => MapEffect::Nothing,
        }
    }
}

/// One submitted job, remembered until it reports back.
struct MapJob {
    id: u64,
    effect: MapEffect,
    sources: Vec<PathBuf>,
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

    /// Wakes a dormant warm instance: `WmEvent::Adopted`, or defensively the
    /// first real key/pointer input in case that message was lost. A no-op
    /// past the first call (`Dormancy::wake` only fires once), so input
    /// arriving after `Adopted` already woke it never rescans.
    fn wake(&mut self, cx: &mut Cx) {
        if self.dormancy.wake() {
            log!("files: warm instance woken, scanning now");
            self.enter_tab(cx);
        }
    }

    // ------------------------------------------------------------------ chat

    /// Open or close the ask panel. Opening it for the first time is what
    /// starts the model loading — until then this app has no idea a language
    /// model exists, which is the only way a file browser is allowed to have
    /// one. Cmd+K, or the speech-bubble button in the toolbar.
    #[cfg(feature = "chat")]
    fn toggle_chat(&mut self, cx: &mut Cx) {
        if vfs().is_demo() {
            self.status(cx, "Chat is not in this demo");
            return;
        }
        let open = !self.chat_open;
        self.chat_open = open;
        self.ui.widget(cx, ids!(chat_panel)).set_visible(cx, open);
        if !open {
            return;
        }
        if self.agent.is_none() {
            self.start_chat(cx);
        }
        self.refresh_chat(cx);
        self.focus_soon(cx, FocusTarget::Chat);
    }

    /// Load the model, once. A machine without the weights on it says so and
    /// carries on being a file browser.
    #[cfg(feature = "chat")]
    fn start_chat(&mut self, cx: &mut Cx) {
        let Some(model) = chat_agent::model_path() else {
            self.chat.push(
                ChatVoice::Info,
                format!(
                    "No local model on this machine. Put a Qwen GGUF at {} (or point {} at one) and reopen this panel.",
                    chat_agent::MODEL_FILE,
                    chat_agent::MODEL_ENV,
                ),
            );
            self.set_chat_status(cx, "no model — the rest of the app is unaffected");
            self.redraw_chat(cx);
            return;
        };
        self.agent = Some(ChatAgent::start(
            model.clone(),
            CHAT_SYSTEM_PROMPT.to_string(),
            chat_tools::tools(),
        ));
        self.tool_runner = Some(ToolRunner::new());
        self.chat.push(
            ChatVoice::Info,
            format!("Loading {}…", display_name(&model)),
        );
        self.set_chat_status(cx, "loading the model…");
        self.redraw_chat(cx);
    }

    #[cfg(feature = "chat")]
    fn set_chat_status(&mut self, cx: &mut Cx, text: &str) {
        if self.chat_status == text {
            return;
        }
        self.chat_status = text.to_string();
        self.ui.label(cx, ids!(chat_status)).set_text(cx, text);
    }

    #[cfg(feature = "chat")]
    fn redraw_chat(&mut self, cx: &mut Cx) {
        let list = self.ui.portal_list(cx, ids!(chat_list));
        list.set_tail_range(true);
        list.redraw(cx);
    }

    /// Where the user is, as the model reads it: the folder, the view, and
    /// what is picked. This rides in front of every question and never appears
    /// in the transcript — "what is this?" is the whole of what was asked.
    #[cfg(feature = "chat")]
    fn chat_where(&mut self, cx: &mut Cx) -> String {
        let mode = self.tabs[self.tab].mode;
        let dir = self.current_dir();
        let mut out = format!(
            "[where the user is]\nhome: {}\nfolder: {}\nview: {}\n",
            self.home.display(),
            dir.display(),
            mode.label(),
        );
        if mode.is_treemap() {
            let map = self.with_contents(cx, |contents, cx| {
                let map = contents.treemap(cx);
                (map.selection(), map.status())
            });
            let (picked, status) = map.unwrap_or_default();
            out.push_str(&format!("map: {status}\n"));
            match picked {
                Some(path) => out.push_str(&format!("selected: {}\n", describe_path(&path))),
                None => out.push_str("selected: nothing on the map is picked\n"),
            }
            return out;
        }
        let selected = self
            .with_contents(cx, |contents, _| contents.selected_entries())
            .unwrap_or_default();
        if selected.is_empty() {
            out.push_str("selected: nothing — the question is about the folder itself\n");
            return out;
        }
        out.push_str(&format!("selected: {} item(s)\n", selected.len()));
        for entry in selected.iter().take(12) {
            out.push_str(&format!(
                "  {} — {}, {}\n",
                entry.path.display(),
                entry.kind_text(),
                entry.size_text(),
            ));
        }
        if selected.len() > 12 {
            out.push_str(&format!("  …and {} more\n", selected.len() - 12));
        }
        out
    }

    /// The one-line "about:" chip over the input, and the map strip's hint and
    /// button states. Called from `report`, so it follows every selection
    /// change — and only touches a widget when its text actually changed.
    fn refresh_chat(&mut self, cx: &mut Cx) {
        self.refresh_ai_context(cx);
        let mode = self.tabs[self.tab].mode;
        let picked = self.chat_subject(cx);
        #[cfg(feature = "chat")]
        if self.chat_open {
            let about = match &picked {
                Some(path) => format!("about: {}", describe_path(path)),
                None => format!("about: {} (this folder)", self.current_dir().display()),
            };
            if about != self.chat_about {
                self.chat_about = about.clone();
                self.ui
                    .label(cx, ids!(chat_about_label))
                    .set_text(cx, &about);
            }
        }
        if !mode.is_treemap() {
            return;
        }
        let note = match &picked {
            Some(path) => format!("Rescan · act on {}", display_name(path)),
            None => "Rescan · click a rectangle to pick what to delete".to_string(),
        };
        if note == self.map_tools_note {
            return;
        }
        self.map_tools_note = note.clone();
        self.ui
            .label(cx, ids!(map_tools_hint))
            .set_text(cx, &note);
        // The two delete buttons go out when there is nothing under them: a
        // button that looks live and does nothing is worse than a dim one.
        let palette = Palette::shared();
        let live = Palette::vec4(&palette.fg);
        let danger = Palette::vec4(&palette.danger);
        let dead = Palette::vec4(&palette.muted);
        let has_pick = picked.is_some();
        for (id, lit) in [
            (ids!(map_trash_icon), if has_pick { live } else { dead }),
            (ids!(map_erase_icon), if has_pick { danger } else { dead }),
        ] {
            let mut icon = self.ui.widget(cx, id);
            script_apply_eval!(cx, icon, {
                draw_icon +: {color: #(lit)}
            });
        }
    }

    /// Is the caret in the ask field? The panel stays open while its answer is
    /// read, so "open" cannot be what decides whether a key is text.
    #[cfg(feature = "chat")]
    fn chat_is_typing(&mut self, cx: &mut Cx) -> bool {
        if !self.chat_open {
            return false;
        }
        let area = self.ui.text_input(cx, ids!(chat_input)).area();
        !area.is_empty() && cx.has_key_focus(area)
    }

    #[cfg(not(feature = "chat"))]
    fn chat_is_typing(&mut self, _cx: &mut Cx) -> bool {
        false
    }

    /// Whether the caret is in the filter sidebar's query field.
    fn filter_is_typing(&mut self, cx: &mut Cx) -> bool {
        let area = self.ui.text_input(cx, ids!(filter_query)).area();
        !area.is_empty() && cx.has_key_focus(area)
    }

    /// What "this" means right now: the map's pick on the map, the listing's
    /// selection anywhere else.
    fn chat_subject(&mut self, cx: &mut Cx) -> Option<PathBuf> {
        if self.tabs[self.tab].mode.is_treemap() {
            return self
                .with_contents(cx, |contents, cx| contents.treemap(cx).selection())
                .flatten();
        }
        self.with_contents(cx, |contents, _| contents.selected_entry())
            .flatten()
            .map(|entry| entry.path)
    }

    #[cfg(feature = "chat")]
    fn send_chat(&mut self, cx: &mut Cx) {
        let field = self.ui.text_input(cx, ids!(chat_input));
        let text = field.text().trim().to_string();
        drop(field);
        if text.is_empty() {
            return;
        }
        if self.agent.is_none() {
            self.start_chat(cx);
            if self.agent.is_none() {
                return;
            }
        }
        if !self.chat_ready {
            self.chat
                .push(ChatVoice::Info, "The model is still loading — one moment.");
            self.redraw_chat(cx);
            return;
        }
        if self.chat_busy {
            // A second question while the first is running is an override, not
            // a queue: stop the old one and ask the new one.
            self.stop_chat(cx);
        }
        self.ui.text_input(cx, ids!(chat_input)).set_text(cx, "");
        self.chat.push(ChatVoice::User, text.clone());
        let prompt = format!("{}\n[question]\n{text}", self.chat_where(cx));
        if let Some(agent) = &self.agent {
            agent.send_user_turn(prompt);
        }
        self.chat_busy = true;
        self.chat_tool_rounds = 0;
        self.chat_awaiting_tools = 0;
        self.chat_tool_replies.clear();
        self.set_chat_status(cx, "thinking…");
        self.set_chat_running(cx, true);
        self.redraw_chat(cx);
    }

    #[cfg(feature = "chat")]
    fn stop_chat(&mut self, cx: &mut Cx) {
        if !self.chat_busy {
            return;
        }
        if let Some(agent) = &self.agent {
            agent.cancel();
        }
        self.chat.commit_pending();
        self.chat.push(ChatVoice::Info, "stopped");
        self.chat_busy = false;
        self.chat_awaiting_tools = 0;
        self.chat_tool_replies.clear();
        self.set_chat_status(cx, "ready");
        self.set_chat_running(cx, false);
        self.redraw_chat(cx);
    }

    /// Swap the Ask button for Stop while a turn is running.
    #[cfg(feature = "chat")]
    fn set_chat_running(&mut self, cx: &mut Cx, running: bool) {
        self.ui
            .widget(cx, ids!(chat_send))
            .set_visible(cx, !running);
        self.ui.widget(cx, ids!(chat_stop)).set_visible(cx, running);
    }

    /// Everything the model and the tool worker have said since the last frame.
    #[cfg(feature = "chat")]
    fn drain_chat(&mut self, cx: &mut Cx) {
        let events = match &self.agent {
            Some(agent) => agent.poll(),
            None => Vec::new(),
        };
        for event in events {
            self.on_chat_event(cx, event);
        }
        let replies = match &self.tool_runner {
            Some(runner) => runner.drain(),
            None => Vec::new(),
        };
        for reply in replies {
            self.chat.push(
                ChatVoice::Tool,
                if reply.is_error {
                    format!("⚠ {}", reply.note)
                } else {
                    reply.note.clone()
                },
            );
            self.chat_tool_replies.push(ToolReply {
                text: reply.text,
                is_error: reply.is_error,
            });
            if self.chat_tool_replies.len() >= self.chat_awaiting_tools.max(1) {
                let results: Vec<(String, bool)> = self
                    .chat_tool_replies
                    .drain(..)
                    .map(|reply| (reply.text, reply.is_error))
                    .collect();
                self.chat_awaiting_tools = 0;
                if let Some(agent) = &self.agent {
                    agent.send_tool_results(results);
                }
                self.set_chat_status(cx, "reading…");
            }
            self.redraw_chat(cx);
        }
    }

    #[cfg(feature = "chat")]
    fn on_chat_event(&mut self, cx: &mut Cx, event: ChatEvent) {
        match event {
            ChatEvent::Loading { phase, fraction } => {
                let text = format!("loading — {phase} {:.0}%", fraction * 100.0);
                self.set_chat_status(cx, &text);
            }
            ChatEvent::Ready {
                prefill_tokens,
                secs,
            } => {
                self.chat_ready = true;
                self.chat.push(
                    ChatVoice::Info,
                    format!("Ready — {prefill_tokens} tokens of prompt in {secs:.1}s."),
                );
                self.set_chat_status(cx, "ready — ask about the folder or the selection");
                self.redraw_chat(cx);
            }
            ChatEvent::Failed(error) => {
                self.chat_ready = false;
                self.chat_busy = false;
                self.agent = None;
                self.chat.push(ChatVoice::Info, format!("⚠ {error}"));
                self.set_chat_status(cx, "the model could not be loaded");
                self.set_chat_running(cx, false);
                self.redraw_chat(cx);
            }
            ChatEvent::Delta(text) => {
                self.chat.pending.push_str(&text);
                self.redraw_chat(cx);
            }
            ChatEvent::ToolCall { name, args } => {
                self.chat.commit_pending();
                self.chat_awaiting_tools += 1;
                let job = ToolJob {
                    name,
                    args,
                    cwd: self.current_dir(),
                    home: self.home.clone(),
                };
                match (&self.tool_runner, self.chat_tool_rounds < MAX_TOOL_ROUNDS) {
                    (Some(runner), true) => runner.submit(job),
                    // Enough. The turn ends with the truth rather than with
                    // another lap of the same three folders.
                    _ => {
                        self.chat_awaiting_tools = self.chat_awaiting_tools.saturating_sub(1);
                        if let Some(agent) = &self.agent {
                            agent.send_tool_results(vec![(
                                "that is enough looking around — answer from what you already have"
                                    .to_string(),
                                true,
                            )]);
                        }
                    }
                }
                self.redraw_chat(cx);
            }
            ChatEvent::TurnDone {
                tool_calls,
                tokens,
                secs,
                context_used,
                context_max,
            } => {
                if tool_calls > 0 {
                    // The tools drive the next round; the turn is not over.
                    self.chat_tool_rounds += 1;
                    self.set_chat_status(cx, "looking…");
                    return;
                }
                self.chat.commit_pending();
                self.chat_busy = false;
                self.set_chat_running(cx, false);
                let rate = tokens as f64 / secs.max(0.001);
                self.set_chat_status(
                    cx,
                    &format!(
                        "{tokens} tokens in {secs:.1}s ({rate:.1} tok/s) · context {context_used}/{context_max}"
                    ),
                );
                self.redraw_chat(cx);
            }
            ChatEvent::ContextFull => {
                self.chat.commit_pending();
                self.chat_busy = false;
                self.set_chat_running(cx, false);
                self.chat.push(
                    ChatVoice::Info,
                    "⚠ this conversation has filled the model's context — reopen the app to start a fresh one",
                );
                self.set_chat_status(cx, "context full");
                self.redraw_chat(cx);
            }
        }
    }

    // ------------------------------------------------------- the map's tools

    /// The map strip's buttons. They act on the picked rectangle through
    /// exactly the paths the keyboard and the context menu already use — the
    /// permanent delete included, which still asks once and acts on the second
    /// press.
    fn handle_map_tool_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if !self.tabs[self.tab].mode.is_treemap() {
            return;
        }
        for (id, projection) in PROJ_BUTTONS {
            if self.ui.view(cx, id).finger_down(actions).is_some() {
                return self.set_projection_choice(cx, projection);
            }
        }
        if self.ui.view(cx, ids!(map_rescan)).finger_down(actions).is_some() {
            return self.rescan_map(cx);
        }
        if self.ui.view(cx, ids!(map_filter)).finger_down(actions).is_some() {
            let open = !self.filter_popup_open;
            return self.set_filter_popup(cx, open);
        }
        if let Some(ignore) = self.ui.check_box(cx, ids!(map_scan_all)).changed(actions) {
            // Checked means today's behaviour: leave the system folders out.
            crate::model::set_scan_all(!ignore);
            self.status(
                cx,
                if ignore {
                    "System folders excluded again — rescanning"
                } else {
                    "Measuring system folders too — macOS will ask permission per folder"
                },
            );
            self.with_contents(cx, |contents, cx| contents.treemap(cx).remap(cx));
            return;
        }
        let trash = self.ui.view(cx, ids!(map_trash)).finger_down(actions).is_some();
        let erase = self.ui.view(cx, ids!(map_erase)).finger_down(actions).is_some();
        if !trash && !erase {
            return;
        }
        if self.chat_subject(cx).is_none() {
            self.status(cx, "Click a rectangle on the map first");
            return;
        }
        if trash {
            self.trash_selection(cx);
        } else {
            self.delete_forever(cx);
        }
    }

    // ------------------------------------------------------- the map filter

    fn set_filter_popup(&mut self, cx: &mut Cx, open: bool) {
        self.filter_popup_open = open;
        model::pref_set("filter_side", if open { "1" } else { "0" });
        self.ui.widget(cx, ids!(map_side)).set_visible(
            cx,
            open && self.tabs[self.tab].mode.is_treemap(),
        );
        if open {
            self.refresh_filter_popup(cx);
            // The field has no area until the sidebar's first frame; focus
            // lands on the frame that gives it one.
            self.focus_soon(cx, FocusTarget::Filter);
        }
        self.ui.redraw(cx);
    }

    /// The legend half of the popup: swatches in the map's own hues, live
    /// byte totals per kind, heaviest first, zero kinds dimmed but present —
    /// it doubles as the map's colour key.
    fn refresh_filter_popup(&mut self, cx: &mut Cx) {
        let totals = self
            .with_contents(cx, |contents, cx| contents.treemap(cx).kind_totals(cx))
            .unwrap_or([0; 16]);
        let mut classes: Vec<(usize, u64)> = (0..7)
            .map(|class| {
                let bytes = class_kind_values(class)
                    .iter()
                    .map(|&kind| totals[kind as usize])
                    .sum();
                (class, bytes)
            })
            .collect();
        classes.sort_by(|a, b| b.1.cmp(&a.1));
        let palette = Palette::shared();
        for (row, &(class, bytes)) in classes.iter().enumerate() {
            self.legend_rows[row] = class;
            let mut widget = self.ui.widget(cx, FILTER_KIND_IDS[row]);
            let selected = self.filter_kinds[class];
            let swatch = palette.kind_color(class);
            let row_bg = if selected {
                let mut tint = Palette::vec4(&palette.accent);
                tint.w = 0.22;
                tint
            } else {
                Vec4f::default()
            };
            let ink = if bytes == 0 && !selected {
                Palette::vec4(&palette.fg_dim)
            } else {
                Palette::vec4(&palette.fg)
            };
            script_apply_eval!(cx, widget, {
                draw_bg +: { color: #(row_bg) }
            });
            let mut swatch_view = widget.widget(cx, ids!(lg_swatch));
            script_apply_eval!(cx, swatch_view, {
                draw_bg +: { color: #(swatch) }
            });
            let mut name = widget.label(cx, ids!(lg_name));
            name.set_text(cx, CLASS_NAMES[class]);
            script_apply_eval!(cx, name, {
                draw_text +: { color: #(ink) }
            });
            widget
                .label(cx, ids!(lg_bytes))
                .set_text(cx, &treemap::format_bytes(bytes));
        }
        self.style_filter_age(cx);
    }

    fn style_filter_age(&mut self, cx: &mut Cx) {
        let palette = Palette::shared();
        for (index, id) in FILTER_AGE_IDS.iter().enumerate() {
            let mut widget = self.ui.widget(cx, id);
            let on = index == self.filter_age;
            let bg = if on {
                let mut tint = Palette::vec4(&palette.accent);
                tint.w = 0.22;
                tint
            } else {
                Vec4f::default()
            };
            let ink = if on {
                Palette::vec4(&palette.fg_bright)
            } else {
                Palette::vec4(&palette.fg_dim)
            };
            script_apply_eval!(cx, widget, {
                draw_bg +: { color: #(bg) }
            });
            let mut label = widget.label(cx, ids!(chip_label));
            script_apply_eval!(cx, label, {
                draw_text +: { color: #(ink) }
            });
        }
    }

    /// Everything the popup says, folded into one query and applied live.
    fn rebuild_filter(&mut self, cx: &mut Cx) {
        let text = self.ui.text_input(cx, ids!(filter_query)).text();
        let now_min = now_minutes();
        let mut query = treemap::Query::parse(&text, now_min);
        let slid = self.ui.slider(cx, ids!(filter_size)).value().unwrap_or(0.0);
        match slider_bytes(slid) {
            Some(bytes) => {
                query.min_size = Some(query.min_size.map_or(bytes, |q| q.max(bytes)));
                self.ui.label(cx, ids!(filter_size_label)).set_text(
                    cx,
                    &format!("bigger than {}", treemap::format_bytes(bytes)),
                );
            }
            None => {
                self.ui
                    .label(cx, ids!(filter_size_label))
                    .set_text(cx, "any size");
            }
        }
        if self.filter_age > 0 {
            let cutoff = now_min.saturating_sub(AGE_MINUTES[self.filter_age]);
            query.newer_than = Some(query.newer_than.map_or(cutoff, |q| q.max(cutoff)));
        }
        if self.filter_kinds.iter().any(|&on| on) {
            let mask = (0..7)
                .filter(|&class| self.filter_kinds[class])
                .fold(0u16, |mask, class| mask | class_kinds_mask(class));
            query.kinds = Some(mask);
        }
        self.with_contents(cx, |contents, cx| {
            contents.treemap(cx).set_filter(cx, Some(query));
        });
    }

    /// Show every control cleared — the map itself is already unfiltered.
    fn reset_filter_controls(&mut self, cx: &mut Cx) {
        self.filter_age = 0;
        self.filter_kinds = [false; 7];
        self.ui.text_input(cx, ids!(filter_query)).set_text(cx, "");
        self.ui.slider(cx, ids!(filter_size)).set_value(cx, 0.0);
        self.ui
            .label(cx, ids!(filter_size_label))
            .set_text(cx, "any size");
        if self.filter_popup_open {
            self.refresh_filter_popup(cx);
        }
    }

    fn handle_filter_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if !self.filter_popup_open {
            return;
        }
        let mut dirty = false;
        if self
            .ui
            .text_input(cx, ids!(filter_query))
            .changed(actions)
            .is_some()
        {
            dirty = true;
        }
        if self.ui.slider(cx, ids!(filter_size)).slided(actions).is_some() {
            dirty = true;
        }
        for (index, id) in FILTER_AGE_IDS.iter().enumerate() {
            if self.ui.view(cx, id).finger_down(actions).is_some() {
                self.filter_age = index;
                self.style_filter_age(cx);
                dirty = true;
            }
        }
        for row in 0..FILTER_KIND_IDS.len() {
            if self
                .ui
                .view(cx, FILTER_KIND_IDS[row])
                .finger_down(actions)
                .is_some()
            {
                let class = self.legend_rows[row];
                self.filter_kinds[class] = !self.filter_kinds[class];
                self.refresh_filter_popup(cx);
                dirty = true;
            }
        }
        if self.ui.view(cx, ids!(filter_clear)).finger_down(actions).is_some() {
            self.reset_filter_controls(cx);
            self.with_contents(cx, |contents, cx| {
                contents.treemap(cx).set_filter(cx, None);
            });
            return;
        }
        if dirty {
            self.rebuild_filter(cx);
        }
    }
}

/// The filter popup's row slots.
const FILTER_AGE_IDS: [&[LiveId]; 6] = [
    ids!(filter_age0),
    ids!(filter_age1),
    ids!(filter_age2),
    ids!(filter_age3),
    ids!(filter_age4),
    ids!(filter_age5),
];
const FILTER_KIND_IDS: [&[LiveId]; 7] = [
    ids!(filter_kind0),
    ids!(filter_kind1),
    ids!(filter_kind2),
    ids!(filter_kind3),
    ids!(filter_kind4),
    ids!(filter_kind5),
    ids!(filter_kind6),
];
/// "modified within", in minutes; index 0 is "any age".
const AGE_MINUTES: [u32; 6] = [0, 1_440, 4_320, 10_080, 43_200, 525_600];
const CLASS_NAMES: [&str; 7] =
    ["Video", "Images", "Audio", "Code", "Docs", "Archives", "Other"];

/// The `FileKind`s behind one legend class — the exact inverse of
/// `treemap_view::kind_class`, asserted so in a test below.
fn class_kind_values(class: usize) -> &'static [crate::model::FileKind] {
    use crate::model::FileKind::*;
    match class {
        0 => &[Video],
        1 => &[Image],
        2 => &[Audio],
        3 => &[Code],
        4 => &[Text, Pdf],
        5 => &[Archive],
        _ => &[Generic, Folder],
    }
}

fn class_kinds_mask(class: usize) -> u16 {
    class_kind_values(class)
        .iter()
        .fold(0u16, |mask, &kind| mask | 1 << (kind as u16))
}

/// The size slider's sweep: off at the left edge, then a logarithmic run
/// from 1 KB to 10 GB — the range disk questions actually live in.
fn slider_bytes(value: f64) -> Option<u64> {
    if value <= 0.02 {
        return None;
    }
    let t = ((value - 0.02) / 0.98).clamp(0.0, 1.0);
    Some((1_000.0 * 10f64.powf(7.0 * t)) as u64)
}

/// Now, in whole minutes since the epoch — the clock the age filter runs on.
fn now_minutes() -> u32 {
    (vfs::now_secs() / 60).min(u32::MAX as u64) as u32
}

/// How many times the model may go round the look-then-think loop for one
/// question before it has to answer with what it has.
#[cfg(feature = "chat")]
const MAX_TOOL_ROUNDS: usize = 6;

/// One path, as a sentence: what it is and how big. Reads off the disk, so it
/// is the truth at the moment it is asked rather than whatever a listing
/// remembered.
#[cfg(feature = "chat")]
fn describe_path(path: &Path) -> String {
    match model::entry_at(path) {
        Some(entry) => format!(
            "{} — {}, {}",
            path.display(),
            entry.kind_text(),
            entry.size_text()
        ),
        None => path.display().to_string(),
    }
}

/// What the model is told it is, once, in front of everything else.
#[cfg(feature = "chat")]
const CHAT_SYSTEM_PROMPT: &str = "\
You are the assistant inside files, a file browser. You answer questions \
about the files the person is looking at right now.

Every question arrives behind a [where the user is] block: the folder they \
have open and what they have selected. \"this\", \"it\", \"here\" and \"that\" \
mean whatever is selected — and the folder itself when nothing is.

You can only look. list_dir, read_file, stat and treemap_summary are all you \
have. There is nothing that writes, moves, renames or deletes, so if you are \
asked to change something, say plainly that you cannot and tell them what to \
click instead.

Look before you answer. Never guess what a folder holds or how big it is: \
call a tool and say what it said. One or two calls is usually enough, and \
treemap_summary is the one that answers \"what is taking up the space\".

Answer in a couple of short sentences, or a short list. Sizes in human units. \
No markdown headings and no preamble — say the thing.";

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        // Checked once: a warm-pool instance stays dormant until
        // `WmEvent::Adopted` or a real input wakes it (see `Dormancy`).
        self.dormancy = Dormancy::start(makepad_wm_api::warm_start());
        // The feature build and the native switch install the same closed
        // filesystem before preferences or paths can consult a backend.
        if vfs::demo_requested() {
            vfs::install(Arc::new(demo::DemoVfs::new()));
        }
        // The scan-scope checkbox shows the saved choice from the first
        // frame; checked means the system folders stay out.
        self.ui
            .check_box(cx, ids!(map_scan_all))
            .set_active(cx, !crate::model::scan_all(), Animate::No);
        // The block view's saved rendering and whether its filter sidebar
        // was left open — both come back exactly as they were left.
        self.projection = match model::pref_get("projection").as_deref() {
            Some("ortho") => MapProjection::Ortho,
            Some("persp") => MapProjection::Persp,
            _ => MapProjection::Flat,
        };
        self.filter_popup_open = model::pref_get("filter_side").as_deref() == Some("1");
        self.style_projection_buttons(cx);
        let (sender, receiver) = mpsc::channel();
        self.sender = Some(sender);
        self.receiver = Some(receiver);
        let (size_sender, size_receiver) = mpsc::channel();
        self.size_sender = Some(size_sender);
        self.size_receiver = Some(size_receiver);
        self.ops = if vfs().is_instant() {
            None
        } else {
            Some(Ops::new(Box::new(SignalToUI::set_ui_signal)))
        };
        self.home = vfs().home();
        // The desktop's assistant hears about this instance now — unless it
        // is a warm-pool standby, which waits for `Adopted` so a dormant
        // process never shows up as a running Files.
        if !makepad_wm_api::warm_start() {
            self.open_ai_port(cx);
        }
        // The demo must not write to the real home, so its bookmarks live and
        // die with the window.
        self.bookmarks = if vfs::is_demo() {
            Bookmarks::in_memory(Vec::new())
        } else {
            Bookmarks::load(&crate::model::makepad_home())
        };
        if vfs::is_demo() {
            // Say so where it cannot be missed: a recording of the demo must
            // never be mistaken for a recording of somebody's files.
            self.ui.label(cx, ids!(files_title)).set_text(cx, "Files · Demo");
            #[cfg(feature = "chat")]
            self.ui.widget(cx, ids!(chat_button)).set_visible(cx, false);
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
        // An explicit folder argument wins over Home; wm passes none.
        let start = std::env::args()
            .skip(1)
            .find(|a| !a.starts_with('-'))
            .map(PathBuf::from)
            .filter(|p| vfs().is_dir(p))
            .unwrap_or_else(|| self.home.clone());
        self.tabs = vec![Tab::new(start, ViewMode::Icons)];
        self.tab = 0;
        // Warm and still dormant: no disk scan and no thumbnails until
        // `wake` runs it — see `Dormancy`.
        if self.dormancy.is_dormant() {
            log!("files: warm-start dormant, deferring the initial scan");
        } else {
            self.enter_tab(cx);
        }
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

        // ---- the ask panel
        #[cfg(feature = "chat")]
        {
            if self.ui.view(cx, ids!(chat_button)).finger_down(actions).is_some() {
                self.toggle_chat(cx);
            }
            if self.chat_open {
                if self.ui.view(cx, ids!(chat_close)).finger_down(actions).is_some() {
                    self.toggle_chat(cx);
                    return;
                }
                if self.ui.view(cx, ids!(chat_stop)).finger_down(actions).is_some() {
                    self.stop_chat(cx);
                    return;
                }
                let field = self.ui.text_input(cx, ids!(chat_input));
                let returned = field.returned(actions).is_some();
                drop(field);
                if returned || self.ui.view(cx, ids!(chat_send)).finger_down(actions).is_some() {
                    self.send_chat(cx);
                    return;
                }
            }
        }
        self.handle_map_tool_actions(cx, actions);
        self.handle_filter_actions(cx, actions);
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
        if !vfs::demo_requested() {
            makepad_wm_theme::apply(vm);
        }
        Palette::shared().publish(vm);
        crate::theme::script_mod(vm);
        crate::thumbs::script_mod(vm);
        crate::treemap_view::script_mod(vm);
        crate::contents::script_mod(vm);
        #[cfg(feature = "chat")]
        crate::chat_panel::script_mod(vm);
        #[cfg(not(feature = "chat"))]
        crate::no_chat::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        // Defensive fallback: a lost `Adopted` message must not leave a
        // visibly-adopted, actually-being-used instance dark and unscanned.
        if self.dormancy.is_dormant() && is_wake_input(event) {
            self.wake(cx);
        }
        // A press anywhere outside the context menu's cards closes it. The
        // raw event, on purpose: the widgets underneath swallow presses
        // differently in every view, and the full-window overlay the menu
        // sits in has no background of its own, so it never hits — waiting
        // for a bubbled press is how the menu got stuck open.
        if let Event::MouseDown(press) = event {
            if self.menu_open {
                let card = self.ui.view(cx, ids!(ctx_panel)).area().rect(cx);
                let sub = self.ui.view(cx, ids!(ctx_sub_panel)).area().rect(cx);
                let inside = card.contains(press.abs)
                    || (self.submenu_open && sub.contains(press.abs));
                if !inside {
                    self.close_menu(cx);
                }
            }
        }
        if let Event::Signal = event {
            self.drain_directory_results(cx);
            self.drain_ops(cx);
            self.drain_sizes(cx);
            let map_moved = self
                .with_contents(cx, |contents, cx| {
                    contents.drain_thumbs(cx);
                    contents.treemap(cx).drain(cx)
                })
                .unwrap_or(false);
            if self.tabs.get(self.tab).is_some_and(|t| t.mode.is_treemap()) {
                self.report(cx);
                // The legend's byte totals follow the scan in — this is also
                // what fills a sidebar that came back open from the prefs,
                // which otherwise sat as bare swatches until the first toggle.
                if map_moved && self.filter_popup_open {
                    self.refresh_filter_popup(cx);
                }
            }
            #[cfg(feature = "chat")]
            self.drain_chat(cx);
            self.drain_ai_replies(cx);
            self.preview.poll();
        }
        if let Event::Custom(json) = event {
            if let Some(wm) = makepad_wm_api::WmEvent::parse(json) {
                self.handle_wm_event(cx, &wm);
            }
        }
        // The bus's frames ride the same channel under their own envelope.
        let port_events = match self.ai_port.as_mut() {
            Some(port) => port.handle_event(cx, event),
            None => Vec::new(),
        };
        if !port_events.is_empty() {
            self.on_ai_port_events(cx, port_events);
        }
        if self.focus_next.is_event(event).is_some() {
            self.apply_focus(cx);
        }
        if let Event::KeyDown(key) = event {
            self.handle_key(cx, key);
        }
        // The transcript draws from the chat state, so it rides down the tree
        // as the scope — every other widget in this window ignores it.
        #[cfg(feature = "chat")]
        self.ui.handle_event(cx, event, &mut Scope::with_data(&mut self.chat));
        #[cfg(not(feature = "chat"))]
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[cfg(test)]
mod dormancy_tests {
    use super::*;

    #[test]
    fn non_warm_starts_active() {
        let dormancy = Dormancy::start(false);
        assert_eq!(dormancy, Dormancy::Active);
        assert!(!dormancy.is_dormant());
    }

    #[test]
    fn warm_starts_dormant_and_adopted_wakes_exactly_once() {
        let mut dormancy = Dormancy::start(true);
        assert!(dormancy.is_dormant());
        // Adopted wakes it...
        assert!(dormancy.wake());
        assert!(!dormancy.is_dormant());
        assert_eq!(dormancy, Dormancy::Woken);
        // ...and a second Adopted (or a stray input) never fires again.
        assert!(!dormancy.wake());
        assert_eq!(dormancy, Dormancy::Woken);
    }

    #[test]
    fn waking_an_already_active_instance_is_a_no_op() {
        let mut dormancy = Dormancy::start(false);
        assert!(!dormancy.wake());
        assert_eq!(dormancy, Dormancy::Active);
    }

    #[test]
    fn key_and_pointer_events_are_wake_input() {
        assert!(is_wake_input(&Event::KeyDown(KeyEvent::default())));
        assert!(is_wake_input(&Event::MouseDown(MouseDownEvent {
            abs: dvec2(0.0, 0.0),
            button: MouseButton::PRIMARY,
            window_id: WindowId(0, 0),
            modifiers: KeyModifiers::default(),
            handled: std::cell::Cell::new(Area::default()),
            time: 0.0,
        })));
        // Touch ("finger") input wakes it too — same match arm as the mouse
        // and keyboard cases above; `TouchUpdateEvent` is not part of the
        // widgets crate's public re-export surface so it is not
        // constructible from an app crate's test.
        // A timer tick or a signal drain is not a human touching the app.
        assert!(!is_wake_input(&Event::Signal));
    }

    #[test]
    fn input_wakes_a_dormant_instance_the_same_as_adopted() {
        let mut dormancy = Dormancy::start(true);
        assert!(is_wake_input(&Event::KeyDown(KeyEvent::default())));
        assert!(dormancy.wake());
        assert!(!dormancy.is_dormant());
    }

    // The legend's classes and the map's kind_class must be exact inverses,
    // or a chip would tint tiles it cannot filter.
    #[test]
    fn every_kind_belongs_to_the_class_that_claims_it() {
        use crate::model::FileKind;
        for kind in [
            FileKind::Folder,
            FileKind::Image,
            FileKind::Text,
            FileKind::Code,
            FileKind::Audio,
            FileKind::Video,
            FileKind::Archive,
            FileKind::Pdf,
            FileKind::Generic,
        ] {
            let class = crate::treemap_view::kind_class(kind) as usize;
            assert!(
                class_kind_values(class).contains(&kind),
                "{kind:?} paints as class {class} but the legend chip for it filters {:?}",
                class_kind_values(class),
            );
            assert!(class_kinds_mask(class) & (1 << (kind as u16)) != 0);
        }
    }
}
