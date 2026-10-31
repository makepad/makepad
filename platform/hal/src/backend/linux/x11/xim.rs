//! XIM style negotiation and preedit callbacks ported from xlib_app.rs:30-467.
use super::sys as x11_sys;
use std::{
    cell::RefCell,
    ffi::{c_char, c_int, c_ulong, c_void},
    mem, ptr,
};
thread_local! {
    // Accumulates the XIM on-the-spot preedit (composition) string. The Xlib
    // callbacks below run synchronously inside `XFilterEvent`, where the event
    // loop already holds `&mut XlibApp`; to avoid re-entrant access they only
    // touch this thread-local, and the loop drains it into the focused TextInput
    // right after `XFilterEvent` returns.
    static XIM_PREEDIT: RefCell<XimPreedit> = RefCell::new(XimPreedit {
        text: String::new(),
        dirty: false,
    });
}

struct XimPreedit {
    text: String,
    dirty: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum XimPreeditStyle {
    Callback,
    Position,
    Nothing,
}

#[derive(Clone, Copy)]
pub struct XimInputContext {
    pub xic: x11_sys::XIC,
    pub input_style: c_ulong,
    pub preedit_style: XimPreeditStyle,
    pub spot_initialized_at_creation: bool,
}

impl XimInputContext {
    pub fn status_style(self) -> c_ulong {
        let preedit_mask = (x11_sys::XIMPreeditCallbacks
            | x11_sys::XIMPreeditPosition
            | x11_sys::XIMPreeditNothing) as c_ulong;
        self.input_style & !preedit_mask
    }
}

// Splice `insert` into `buf`, replacing `char_len` characters starting at
// character index `char_first` (XIM preedit change indices are in characters).
fn xim_preedit_replace(buf: &mut String, char_first: usize, char_len: usize, insert: &str) {
    let byte_start = buf
        .char_indices()
        .nth(char_first)
        .map(|(i, _)| i)
        .unwrap_or(buf.len());
    let byte_end = buf
        .char_indices()
        .nth(char_first + char_len)
        .map(|(i, _)| i)
        .unwrap_or(buf.len());
    if byte_start <= byte_end {
        buf.replace_range(byte_start..byte_end, insert);
    }
}

unsafe fn query_xim_supported_styles(xim: x11_sys::XIM) -> Option<Vec<c_ulong>> {
    let mut styles_ptr: *mut x11_sys::XIMStyles = ptr::null_mut();
    let failed_attr = x11_sys::XGetIMValues(
        xim,
        x11_sys::XNQueryInputStyle.as_ptr(),
        &mut styles_ptr,
        ptr::null_mut::<c_void>(),
    );
    if !failed_attr.is_null() || styles_ptr.is_null() {
        if !styles_ptr.is_null() {
            x11_sys::XFree(styles_ptr as *mut c_void);
        }
        return None;
    }

    let styles = &*styles_ptr;
    let supported = if styles.supported_styles.is_null() {
        Vec::new()
    } else {
        std::slice::from_raw_parts(styles.supported_styles, styles.count_styles as usize)
            .iter()
            .map(|style| *style as c_ulong)
            .collect()
    };
    x11_sys::XFree(styles_ptr as *mut c_void);
    Some(supported)
}

fn xim_style_supported(styles: Option<&[c_ulong]>, style: c_ulong) -> bool {
    styles
        .map(|styles| styles.iter().any(|supported| *supported == style))
        .unwrap_or(true)
}

pub fn xim_status_candidates() -> [c_ulong; 2] {
    [
        x11_sys::XIMStatusNothing as c_ulong,
        x11_sys::XIMStatusNone as c_ulong,
    ]
}

pub(super) unsafe fn create_xim_font_set(display: *mut x11_sys::Display) -> x11_sys::XFontSet {
    let font_names: &[&[u8]] = &[
        b"-misc-fixed-medium-r-normal--14-*-*-*-*-*-*-*\0",
        b"fixed\0",
        b"*\0",
    ];
    for font_name in font_names {
        let mut missing_list: *mut *mut c_char = ptr::null_mut();
        let mut missing_count: c_int = 0;
        let mut default_string: *mut c_char = ptr::null_mut();
        let font_set = x11_sys::XCreateFontSet(
            display,
            font_name.as_ptr() as *const c_char,
            &mut missing_list,
            &mut missing_count,
            &mut default_string,
        );
        if !missing_list.is_null() {
            x11_sys::XFreeStringList(missing_list);
        }
        if !font_set.is_null() {
            return font_set;
        }
    }
    ptr::null_mut()
}

// XIM preedit start: reset our buffer and report "no length limit" (-1).
unsafe extern "C" fn xim_preedit_start(
    _ic: x11_sys::XIC,
    _client: x11_sys::XPointer,
    _call: x11_sys::XPointer,
) -> c_int {
    XIM_PREEDIT.with(|p| {
        let mut p = p.borrow_mut();
        p.text.clear();
        p.dirty = false;
    });
    -1
}

// XIM preedit done: composition finished or was cancelled; clear the preview.
unsafe extern "C" fn xim_preedit_done(
    _ic: x11_sys::XIC,
    _client: x11_sys::XPointer,
    _call: x11_sys::XPointer,
) {
    XIM_PREEDIT.with(|p| {
        let mut p = p.borrow_mut();
        if !p.text.is_empty() {
            p.text.clear();
            p.dirty = true;
        }
    });
}

// XIM preedit caret: the caret moved within the preedit. We render the whole
// preedit inline with the caret at its end, so there is nothing to do here.
unsafe extern "C" fn xim_preedit_caret(
    _ic: x11_sys::XIC,
    _client: x11_sys::XPointer,
    _call: x11_sys::XPointer,
) {
}

// XIM preedit draw: the IM edits the preedit string in place. Splice the change
// into our buffer and flag it so the event loop forwards the new preedit.
unsafe extern "C" fn xim_preedit_draw(
    _ic: x11_sys::XIC,
    _client: x11_sys::XPointer,
    call: x11_sys::XPointer,
) {
    if call.is_null() {
        return;
    }
    let draw = &*(call as *const x11_sys::XIMPreeditDrawCallbackStruct);
    let chg_first = draw.chg_first.max(0) as usize;
    let chg_length = draw.chg_length.max(0) as usize;
    let insert = if draw.text.is_null() {
        // Null text means a pure deletion of `chg_length` characters.
        String::new()
    } else {
        let text = &*draw.text;
        if text.string.is_null() || text.encoding_is_wchar != 0 {
            // We only handle the multi-byte (locale-encoded, typically UTF-8)
            // form; the string is NUL-terminated per Xlib.
            String::new()
        } else {
            std::ffi::CStr::from_ptr(text.string)
                .to_string_lossy()
                .into_owned()
        }
    };
    XIM_PREEDIT.with(|p| {
        let mut p = p.borrow_mut();
        xim_preedit_replace(&mut p.text, chg_first, chg_length, &insert);
        p.dirty = true;
    });
}

unsafe fn create_xim_callback_input_context(
    xim: x11_sys::XIM,
    window: x11_sys::Window,
    status_style: c_ulong,
) -> Option<XimInputContext> {
    let start_cb = x11_sys::XIMCallback {
        client_data: ptr::null_mut(),
        // The preedit-start callback returns an int (max length); transmute its
        // signature into the void-returning XIMProc the struct field expects.
        callback: mem::transmute::<
            unsafe extern "C" fn(x11_sys::XIC, x11_sys::XPointer, x11_sys::XPointer) -> c_int,
            x11_sys::XIMProc,
        >(xim_preedit_start),
    };
    let draw_cb = x11_sys::XIMCallback {
        client_data: ptr::null_mut(),
        callback: Some(xim_preedit_draw),
    };
    let done_cb = x11_sys::XIMCallback {
        client_data: ptr::null_mut(),
        callback: Some(xim_preedit_done),
    };
    let caret_cb = x11_sys::XIMCallback {
        client_data: ptr::null_mut(),
        callback: Some(xim_preedit_caret),
    };

    // Xlib copies the callback values into the IC, so these locals only need to
    // outlive XCreateIC.
    let preedit_attr = x11_sys::XVaCreateNestedList(
        0,
        x11_sys::XNPreeditStartCallback.as_ptr(),
        &start_cb,
        x11_sys::XNPreeditDrawCallback.as_ptr(),
        &draw_cb,
        x11_sys::XNPreeditDoneCallback.as_ptr(),
        &done_cb,
        x11_sys::XNPreeditCaretCallback.as_ptr(),
        &caret_cb,
        ptr::null_mut::<c_void>(),
    );
    if preedit_attr.is_null() {
        return None;
    }

    let input_style = x11_sys::XIMPreeditCallbacks as c_ulong | status_style;
    let xic = x11_sys::XCreateIC(
        xim,
        x11_sys::XNInputStyle.as_ptr(),
        input_style,
        x11_sys::XNClientWindow.as_ptr(),
        window,
        x11_sys::XNFocusWindow.as_ptr(),
        window,
        x11_sys::XNPreeditAttributes.as_ptr(),
        preedit_attr,
        ptr::null_mut::<c_void>(),
    );
    x11_sys::XFree(preedit_attr);
    if xic.is_null() {
        None
    } else {
        Some(XimInputContext {
            xic,
            input_style,
            preedit_style: XimPreeditStyle::Callback,
            spot_initialized_at_creation: false,
        })
    }
}

unsafe fn create_xim_position_input_context_internal(
    xim: x11_sys::XIM,
    window: x11_sys::Window,
    status_style: c_ulong,
    font_set: x11_sys::XFontSet,
    initial_ime_area: Option<(x11_sys::XPoint, x11_sys::XRectangle)>,
) -> Option<XimInputContext> {
    let default_spot = x11_sys::XPoint { x: 0, y: 0 };
    let preedit_attr = match (initial_ime_area, font_set.is_null()) {
        (Some((spot, area)), false) => x11_sys::XVaCreateNestedList(
            0,
            x11_sys::XNSpotLocation.as_ptr(),
            &spot,
            x11_sys::XNArea.as_ptr(),
            &area,
            x11_sys::XNFontSet.as_ptr(),
            font_set,
            ptr::null_mut::<c_void>(),
        ),
        (Some((spot, area)), true) => x11_sys::XVaCreateNestedList(
            0,
            x11_sys::XNSpotLocation.as_ptr(),
            &spot,
            x11_sys::XNArea.as_ptr(),
            &area,
            ptr::null_mut::<c_void>(),
        ),
        (None, false) => x11_sys::XVaCreateNestedList(
            0,
            x11_sys::XNSpotLocation.as_ptr(),
            &default_spot,
            x11_sys::XNFontSet.as_ptr(),
            font_set,
            ptr::null_mut::<c_void>(),
        ),
        (None, true) => x11_sys::XVaCreateNestedList(
            0,
            x11_sys::XNSpotLocation.as_ptr(),
            &default_spot,
            ptr::null_mut::<c_void>(),
        ),
    };
    if preedit_attr.is_null() {
        return None;
    }

    let input_style = x11_sys::XIMPreeditPosition as c_ulong | status_style;
    let xic = x11_sys::XCreateIC(
        xim,
        x11_sys::XNInputStyle.as_ptr(),
        input_style,
        x11_sys::XNClientWindow.as_ptr(),
        window,
        x11_sys::XNFocusWindow.as_ptr(),
        window,
        x11_sys::XNPreeditAttributes.as_ptr(),
        preedit_attr,
        ptr::null_mut::<c_void>(),
    );
    x11_sys::XFree(preedit_attr);
    if xic.is_null() {
        None
    } else {
        Some(XimInputContext {
            xic,
            input_style,
            preedit_style: XimPreeditStyle::Position,
            spot_initialized_at_creation: initial_ime_area.is_some(),
        })
    }
}

unsafe fn create_xim_position_input_context(
    font_set: x11_sys::XFontSet,
    xim: x11_sys::XIM,
    window: x11_sys::Window,
    status_style: c_ulong,
) -> Option<XimInputContext> {
    create_xim_position_input_context_internal(xim, window, status_style, font_set, None)
}

pub unsafe fn create_xim_position_input_context_with_spot(
    font_set: x11_sys::XFontSet,
    xim: x11_sys::XIM,
    window: x11_sys::Window,
    status_style: c_ulong,
    spot: x11_sys::XPoint,
    area: x11_sys::XRectangle,
) -> Option<XimInputContext> {
    create_xim_position_input_context_internal(
        xim,
        window,
        status_style,
        font_set,
        Some((spot, area)),
    )
}

unsafe fn create_xim_nothing_input_context(
    xim: x11_sys::XIM,
    window: x11_sys::Window,
    status_style: c_ulong,
) -> Option<XimInputContext> {
    let input_style = x11_sys::XIMPreeditNothing as c_ulong | status_style;
    let xic = x11_sys::XCreateIC(
        xim,
        x11_sys::XNInputStyle.as_ptr(),
        input_style,
        x11_sys::XNClientWindow.as_ptr(),
        window,
        x11_sys::XNFocusWindow.as_ptr(),
        window,
        ptr::null_mut::<c_void>(),
    );
    if xic.is_null() {
        None
    } else {
        Some(XimInputContext {
            xic,
            input_style,
            preedit_style: XimPreeditStyle::Nothing,
            spot_initialized_at_creation: false,
        })
    }
}

/// Creates an input context for `window`.
///
/// Prefer standard over-the-spot (`XIMPreeditPosition`) because XIM only
/// specifies `XNSpotLocation` for that style. Callback/on-the-spot spot
/// forwarding was added to libX11 later and remains implementation-defined, so
/// it is only used when position style is unavailable. Committed text still
/// arrives through `Xutf8LookupString` in every supported style.
pub unsafe fn create_xim_input_context(
    font_set: x11_sys::XFontSet,
    xim: x11_sys::XIM,
    window: x11_sys::Window,
) -> Option<XimInputContext> {
    if xim.is_null() {
        return None;
    }

    let supported_styles = query_xim_supported_styles(xim);
    let supported_styles = supported_styles.as_deref();

    for status_style in xim_status_candidates() {
        let input_style = x11_sys::XIMPreeditPosition as c_ulong | status_style;
        if !xim_style_supported(supported_styles, input_style) {
            continue;
        }
        if let Some(position_context) =
            create_xim_position_input_context(font_set, xim, window, status_style)
        {
            return Some(position_context);
        }
    }

    for status_style in xim_status_candidates() {
        let input_style = x11_sys::XIMPreeditCallbacks as c_ulong | status_style;
        if !xim_style_supported(supported_styles, input_style) {
            continue;
        }
        if let Some(callback_context) = create_xim_callback_input_context(xim, window, status_style)
        {
            return Some(callback_context);
        }
    }

    for status_style in xim_status_candidates() {
        let input_style = x11_sys::XIMPreeditNothing as c_ulong | status_style;
        if !xim_style_supported(supported_styles, input_style) {
            continue;
        }
        if let Some(nothing_context) = create_xim_nothing_input_context(xim, window, status_style) {
            return Some(nothing_context);
        }
    }

    None
}

pub(super) fn take_preedit() -> Option<String> {
    XIM_PREEDIT.with(|p| {
        let mut p = p.borrow_mut();
        if !p.dirty {
            return None;
        }
        p.dirty = false;
        Some(p.text.clone())
    })
}
