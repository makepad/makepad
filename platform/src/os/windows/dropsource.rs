#![allow(dead_code)]
use crate::{
    log,
    implement_com,
    windows::{
        core,
        Win32::{
            System::{
                Ole::{
                    IDropSource,
                    IDropSource_Impl,
                    DROPEFFECT,
                },
                SystemServices::{
                    MODIFIERKEYS_FLAGS,
                    MK_LBUTTON,
                },
            },
            Foundation::{
                BOOL,
                S_OK,
                DRAGDROP_S_DROP,
                DRAGDROP_S_USEDEFAULTCURSORS,
            },
        },
    },
};

pub struct DropSource {

}

implement_com!{
    for_struct: DropSource,
    identity: IDropSource,
    wrapper_struct: DropSource_Com,
    interface_count: 1,
    interfaces: {
        0: IDropSource
    }
}

#[allow(non_snake_case)]
impl IDropSource_Impl for DropSource {
    fn QueryContinueDrag(&self, fescapepressed: BOOL, grfkeystate: MODIFIERKEYS_FLAGS) -> core::HRESULT {
        if (grfkeystate & MK_LBUTTON) == MODIFIERKEYS_FLAGS(0) {
            DRAGDROP_S_DROP
        }
        else {
            S_OK
        }
    }

    fn GiveFeedback(&self, dweffect: DROPEFFECT) -> core::HRESULT {
        //log!("DropSource::GiveFeedback");
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}
