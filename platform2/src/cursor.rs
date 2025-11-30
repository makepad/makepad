use {
    crate::{
        makepad_micro_serde::*,
        makepad_script::*,
        //cx::Cx,
    }
};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Script, ScriptHook, SerBin, DeBin, SerJson, DeJson)]
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