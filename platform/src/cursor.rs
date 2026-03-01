use crate::{
    makepad_micro_serde::*,
    makepad_script::*,
    //cx::Cx,
};

// Note: Using manual SerJson/DeJson impl with integer encoding to reduce code bloat
// (derive-based string matching generates ~2500 lines of LLVM IR for 26 variants)
#[derive(Clone, Copy, Debug, Hash, PartialEq, Script, ScriptHook, SerBin, DeBin)]
pub enum MouseCursor {
    // don't show the cursor
    Hidden,

    //  *
    //  *  *
    //  *    *
    //  *      *
    //  *   *
    //  *    *
    //        *
    #[pick]
    Default,

    //     |
    //     |
    //  ---+---
    //     |
    //     |
    Crosshair,

    //    *
    //    *
    //    * * * *
    // *  * * * *
    // *  *     *
    //  * *     *
    //  *      *
    Hand,

    //  *
    //  *  *
    //  *    *
    //  *      *
    //  *   *
    //  *    *
    //        *
    Arrow,

    //     ^
    //     |
    //  <--+-->
    //     |
    //     v
    Move,

    //   --+--
    //     |
    //     |
    //   __|__
    Text,

    //  |******|
    //   \****/
    //    \**/
    //    /**\
    //   /****\
    //  |******|
    Wait,

    //  *
    //  *  *
    //  *    *
    //  *      *
    //  *   *
    //  *    *   ?
    //        *
    Help,

    //    _____
    //   / \   \
    //  |   \  |
    //   \___\/
    NotAllowed,

    //
    //    * * * *
    //    * * * *
    // *  * * * *
    // *  *     *
    //  * *     *
    //  *      *
    Grab,

    //
    //
    //    * * * *
    //  * * * * *
    // *  *     *
    //  * *     *
    //  *      *
    Grabbing,

    /*

    //  *
    //  *  *
    //  *    *
    //  *      * |----|
    //  *   *     \--/
    //  *    *    /--\
    //        *  |----|
    Progress,

    //  *
    //  *  *
    //  *    *
    //  *      *
    //  *   *   |----|
    //  *    *  |----|
    //        * |----|
    ContextMenu,

    //     | |
    //     | |
    //  ---+ +---
    //  ---+ +---
    //     | |
    //     | |

    Cell,
    //   |     |
    //   |-----|
    //   |     |
    VerticalText,

    //  *
    //  *  *
    //  *    *
    //  *      *
    //  *   *    |  ^ |
    //  *    *   | /  |
    //        *
    Alias,

    //  *
    //  *  *
    //  *    *
    //  *      *
    //  *   *
    //  *    *   |+|
    //        *
    Copy,

    //    *
    //    *
    //    * * * *
    // *  * * * *    _____
    // *  *     *   / \   \
    //  * *     *  |   \  |
    //  *      *    \___\/
    NoDrop,

    //     ^
    //   < * >
    //     v
    AllScroll,

    //   _____
    //  /  |  \
    //  | -+- |
    //  \__|__/
    //     |
    //     |
    ZoomIn,

    //   _____
    //  /     \
    //  | --- |
    //  \_____/
    //     |
    //     |
    ZoomOut,
    */
    //     ^
    //     |
    NResize,

    //     ^
    //    /
    NeResize,

    //    -->
    EResize,

    //    \
    //     v
    SeResize,

    //     |
    //     v
    SResize,

    //    /
    //   v
    SwResize,

    //    <--
    WResize,

    //   ^
    //    \
    NwResize,

    //     ^
    //     |
    //     v
    NsResize,

    //     ^
    //    /
    //   v
    NeswResize,

    //  <--->
    EwResize,

    //   ^
    //    \
    //     v
    NwseResize,

    //     ||
    //   <-||->
    //     ||
    ColResize,

    //     ^
    //     |
    //   =====
    //     |
    //     v
    RowResize,
}

impl Eq for MouseCursor {}
impl Default for MouseCursor {
    fn default() -> MouseCursor {
        MouseCursor::Default
    }
}

// Const array for efficient index-to-variant conversion
const MOUSECURSOR_VARIANTS: [MouseCursor; 26] = [
    MouseCursor::Hidden,
    MouseCursor::Default,
    MouseCursor::Crosshair,
    MouseCursor::Hand,
    MouseCursor::Arrow,
    MouseCursor::Move,
    MouseCursor::Text,
    MouseCursor::Wait,
    MouseCursor::Help,
    MouseCursor::NotAllowed,
    MouseCursor::Grab,
    MouseCursor::Grabbing,
    MouseCursor::NResize,
    MouseCursor::NeResize,
    MouseCursor::EResize,
    MouseCursor::SeResize,
    MouseCursor::SResize,
    MouseCursor::SwResize,
    MouseCursor::WResize,
    MouseCursor::NwResize,
    MouseCursor::NsResize,
    MouseCursor::NeswResize,
    MouseCursor::EwResize,
    MouseCursor::NwseResize,
    MouseCursor::ColResize,
    MouseCursor::RowResize,
];

// Manual SerJson/DeJson implementations using integer encoding
impl SerJson for MouseCursor {
    fn ser_json(&self, _d: usize, s: &mut SerJsonState) {
        let idx = MOUSECURSOR_VARIANTS
            .iter()
            .position(|c| c == self)
            .unwrap_or(0);
        s.out.push_str(&idx.to_string());
    }
}

impl DeJson for MouseCursor {
    fn de_json(s: &mut DeJsonState, i: &mut std::str::Chars) -> Result<Self, DeJsonErr> {
        let val = u64::de_json(s, i)? as usize;
        Ok(if val < MOUSECURSOR_VARIANTS.len() {
            MOUSECURSOR_VARIANTS[val]
        } else {
            MouseCursor::Default
        })
    }
}

impl From<MouseCursor> for makepad_studio_protocol::MouseCursor {
    fn from(value: MouseCursor) -> Self {
        match value {
            MouseCursor::Hidden => Self::Hidden,
            MouseCursor::Default => Self::Default,
            MouseCursor::Crosshair => Self::Crosshair,
            MouseCursor::Hand => Self::Hand,
            MouseCursor::Arrow => Self::Arrow,
            MouseCursor::Move => Self::Move,
            MouseCursor::Text => Self::Text,
            MouseCursor::Wait => Self::Wait,
            MouseCursor::Help => Self::Help,
            MouseCursor::NotAllowed => Self::NotAllowed,
            MouseCursor::Grab => Self::Grab,
            MouseCursor::Grabbing => Self::Grabbing,
            MouseCursor::NResize => Self::NResize,
            MouseCursor::NeResize => Self::NeResize,
            MouseCursor::EResize => Self::EResize,
            MouseCursor::SeResize => Self::SeResize,
            MouseCursor::SResize => Self::SResize,
            MouseCursor::SwResize => Self::SwResize,
            MouseCursor::WResize => Self::WResize,
            MouseCursor::NwResize => Self::NwResize,
            MouseCursor::NsResize => Self::NsResize,
            MouseCursor::NeswResize => Self::NeswResize,
            MouseCursor::EwResize => Self::EwResize,
            MouseCursor::NwseResize => Self::NwseResize,
            MouseCursor::ColResize => Self::ColResize,
            MouseCursor::RowResize => Self::RowResize,
        }
    }
}

impl From<makepad_studio_protocol::MouseCursor> for MouseCursor {
    fn from(value: makepad_studio_protocol::MouseCursor) -> Self {
        match value {
            makepad_studio_protocol::MouseCursor::Hidden => Self::Hidden,
            makepad_studio_protocol::MouseCursor::Default => Self::Default,
            makepad_studio_protocol::MouseCursor::Crosshair => Self::Crosshair,
            makepad_studio_protocol::MouseCursor::Hand => Self::Hand,
            makepad_studio_protocol::MouseCursor::Arrow => Self::Arrow,
            makepad_studio_protocol::MouseCursor::Move => Self::Move,
            makepad_studio_protocol::MouseCursor::Text => Self::Text,
            makepad_studio_protocol::MouseCursor::Wait => Self::Wait,
            makepad_studio_protocol::MouseCursor::Help => Self::Help,
            makepad_studio_protocol::MouseCursor::NotAllowed => Self::NotAllowed,
            makepad_studio_protocol::MouseCursor::Grab => Self::Grab,
            makepad_studio_protocol::MouseCursor::Grabbing => Self::Grabbing,
            makepad_studio_protocol::MouseCursor::NResize => Self::NResize,
            makepad_studio_protocol::MouseCursor::NeResize => Self::NeResize,
            makepad_studio_protocol::MouseCursor::EResize => Self::EResize,
            makepad_studio_protocol::MouseCursor::SeResize => Self::SeResize,
            makepad_studio_protocol::MouseCursor::SResize => Self::SResize,
            makepad_studio_protocol::MouseCursor::SwResize => Self::SwResize,
            makepad_studio_protocol::MouseCursor::WResize => Self::WResize,
            makepad_studio_protocol::MouseCursor::NwResize => Self::NwResize,
            makepad_studio_protocol::MouseCursor::NsResize => Self::NsResize,
            makepad_studio_protocol::MouseCursor::NeswResize => Self::NeswResize,
            makepad_studio_protocol::MouseCursor::EwResize => Self::EwResize,
            makepad_studio_protocol::MouseCursor::NwseResize => Self::NwseResize,
            makepad_studio_protocol::MouseCursor::ColResize => Self::ColResize,
            makepad_studio_protocol::MouseCursor::RowResize => Self::RowResize,
        }
    }
}
