//! Style constants extracted from Robrix's shared/styles.rs.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.ICON_ADD_USER = crate_resource("self://resources/icons/add_user.svg")
    mod.widgets.ICON_HIERARCHY = crate_resource("self://resources/icons/hierarchy.svg")

    mod.widgets.TITLE_TEXT = theme.font_regular {
        font_size: (13),
    }

    mod.widgets.REGULAR_TEXT = theme.font_regular {
        font_size: (10),
    }

    mod.widgets.COLOR_PRIMARY = #ffffff
    mod.widgets.COLOR_ACTIVE_PRIMARY = #0f88fe
    mod.widgets.COLOR_BG_PREVIEW = #F0F5FF
    mod.widgets.COLOR_AVATAR_BG = #52b2ac
    mod.widgets.COLOR_TEXT = #1C274C

    mod.widgets.COLOR_FG_ACCEPT_GREEN = #138808
    mod.widgets.COLOR_BG_ACCEPT_GREEN = #F0FFF0
    mod.widgets.COLOR_FG_DANGER_RED = #DC0005
    mod.widgets.COLOR_BG_DANGER_RED = #FFF0F0

    mod.widgets.COLOR_NAVIGATION_TAB_BG = #E3E3E3
    mod.widgets.COLOR_NAVIGATION_TAB_BG_HOVER = #C0C0C0
    mod.widgets.COLOR_ACTIVE_PRIMARY_DARKER = #106fcc
}
