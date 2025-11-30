#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
use crate::{
    windows::core::implement,
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

#[implement(IDropSource)]
pub struct DropSource { }
/*
implement_com!{
    for_struct: DropSource,
    identity: IDropSource,
    wrapper_struct: DropSource_Com,
    interface_count: 1,
    interfaces: {
        0: IDropSource
    }
}*/

// IDropSource implementation for DropSource, which validates a drop on left mouse button up

impl IDropSource_Impl for DropSource {

    fn QueryContinueDrag(&self, _: BOOL, grfkeystate: MODIFIERKEYS_FLAGS) -> core::HRESULT {

        // if the left mousebutton is not pressed anymore, drop that item
        if (grfkeystate & MK_LBUTTON) == MODIFIERKEYS_FLAGS(0) {
            DRAGDROP_S_DROP
        }

        else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _: DROPEFFECT) -> core::HRESULT {

        DRAGDROP_S_USEDEFAULTCURSORS
    }
}
