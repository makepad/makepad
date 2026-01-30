use {
    crate::{
        makepad_micro_serde::*,
        makepad_script::*,
        //cx::Cx,
    }
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
    #[pick] Default,
    
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
    MouseCursor::Hidden, MouseCursor::Default, MouseCursor::Crosshair,
    MouseCursor::Hand, MouseCursor::Arrow, MouseCursor::Move, MouseCursor::Text,
    MouseCursor::Wait, MouseCursor::Help, MouseCursor::NotAllowed,
    MouseCursor::Grab, MouseCursor::Grabbing,
    MouseCursor::NResize, MouseCursor::NeResize, MouseCursor::EResize,
    MouseCursor::SeResize, MouseCursor::SResize, MouseCursor::SwResize,
    MouseCursor::WResize, MouseCursor::NwResize, MouseCursor::NsResize,
    MouseCursor::NeswResize, MouseCursor::EwResize, MouseCursor::NwseResize,
    MouseCursor::ColResize, MouseCursor::RowResize,
];

// Manual SerJson/DeJson implementations using integer encoding
impl SerJson for MouseCursor {
    fn ser_json(&self, _d: usize, s: &mut SerJsonState) {
        let idx = MOUSECURSOR_VARIANTS.iter().position(|c| c == self).unwrap_or(0);
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