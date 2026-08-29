use crate::{
    event::DragItem,
    live_id::LiveId,
    log,
    windows::Win32::{
        Foundation::HGLOBAL,
        System::{
            Com::STGMEDIUM,
            Memory::{
                GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_FIXED, GMEM_ZEROINIT,
            },
            Ole::ReleaseStgMedium,
        },
    },
};

// This is where all binary conversion code goes between makepad DragItem and Windows HGLOBAL/DROPFILES structure

/// Decode the DROPFILES block a CF_HDROP medium holds.
///
/// Split out from the HGLOBAL handling so it can be tested without a
/// running Windows message loop: the shape of this block is the only part
/// with any decisions in it, and it is the part that was wrong.
///
/// EVERY name in the block becomes an item. Explorer hands over a whole
/// selection in one drop, and refusing all of them because there was more
/// than one meant a multi-file drag produced no event at all — the window
/// looked broken rather than merely limited.
pub(crate) fn parse_dropfiles(bytes: &[u8]) -> Option<Vec<DragItem>> {
    // fWide lives at byte 16, so anything shorter is not a DROPFILES.
    if bytes.len() < 20 {
        log!("drag object is too short to be a DROPFILES");
        return None;
    }
    let u32_at = |at: usize| {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    };
    let names_offset = u32_at(0) as usize;
    if u32_at(16) == 0 {
        log!("drag object should have wide strings");
        return None;
    }
    // 20 is the plain Windows layout; 28 is ours, with a LiveId wedged in
    // between the header and the names (see create_hglobal_for_dragitem).
    if names_offset != 20 && names_offset != 28 {
        log!("unknown drag object");
        return None;
    }
    if bytes.len() < names_offset {
        log!("drag object is shorter than its own name offset");
        return None;
    }
    let internal_id = if names_offset == 28 {
        let id = LiveId(((u32_at(24) as u64) << 32) | (u32_at(20) as u64));
        (id.0 != 0).then_some(id)
    } else {
        None
    };
    let units: Vec<u16> = bytes[names_offset..]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let mut items = Vec::new();
    // The names are NUL-separated and the list ends at the first empty one
    // (the second NUL of the terminating pair). Decoding a whole name at a
    // time rather than a code unit at a time is what keeps an emoji or any
    // other non-BMP character in a filename intact.
    for name in units.split(|unit| *unit == 0) {
        if name.is_empty() {
            break;
        }
        items.push(DragItem::FilePath {
            path: String::from_utf16_lossy(name),
            internal_id,
        });
    }
    if items.is_empty() {
        return None;
    }
    Some(items)
}

// convert incoming STGMEDIUM from internal or external source to DragItems
pub fn convert_medium_to_dragitems(medium: STGMEDIUM) -> Option<Vec<DragItem>> {
    let hglobal_size = unsafe { GlobalSize(medium.u.hGlobal) };
    let hglobal_raw_ptr = unsafe { GlobalLock(medium.u.hGlobal) };
    let items = {
        let bytes =
            unsafe { std::slice::from_raw_parts(hglobal_raw_ptr as *const u8, hglobal_size) };
        parse_dropfiles(bytes)
    };
    let _ = unsafe { GlobalUnlock(medium.u.hGlobal) };
    unsafe { ReleaseStgMedium(&medium as *const STGMEDIUM as *mut STGMEDIUM) };
    items
}

// create new internal DROPFILES structure from DragItem
pub fn create_hglobal_for_dragitem(drag_item: &DragItem) -> Option<HGLOBAL> {
    if let DragItem::FilePath { path, internal_id } = drag_item {
        // encode filename
        let mut encoded_filename: Vec<u16> = path.encode_utf16().collect();
        encoded_filename.push(0);

        // only one filename
        encoded_filename.push(0);

        // create HGLOBAL to contain DROPFILES structure, the internal ID and this encoded filename
        let size_in_bytes = 28 + encoded_filename.len() * 2;
        let hglobal = unsafe { GlobalAlloc(GMEM_ZEROINIT | GMEM_FIXED, size_in_bytes) }.unwrap();
        let hglobal_raw_ptr = unsafe { GlobalLock(hglobal) };

        // initialize DROPFILES part
        let u32_slice = unsafe { std::slice::from_raw_parts_mut(hglobal_raw_ptr as *mut u32, 7) };
        u32_slice[0] = 28; // offset to filename
        u32_slice[1] = 0;
        u32_slice[2] = 0;
        u32_slice[3] = 0;
        u32_slice[4] = 1; // not 0 because 16-bit characters in the filename

        // initialize internal ID
        if let Some(internal_id) = internal_id {
            u32_slice[6] = (internal_id.0 >> 32) as u32;
            u32_slice[5] = (internal_id.0 & 0xffff_ffff) as u32;
            //let u64_slice = unsafe { std::slice::from_raw_parts_mut((hglobal_raw_ptr as *mut u8).offset(20) as *mut u64,1) };
            //u64_slice[0] = internal_id.0;
        }

        // initialize filename
        unsafe {
            std::ptr::copy_nonoverlapping(
                encoded_filename.as_ptr(),
                (hglobal_raw_ptr as *mut u8).offset(28) as *mut u16,
                encoded_filename.len(),
            )
        };

        // ready
        unsafe { GlobalUnlock(hglobal) }.unwrap();

        Some(hglobal)
    } else {
        log!("only DragItem::FilePath supported");
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a DROPFILES block by hand: `names_offset` is 20 for an
    /// external drop and 28 for one of ours, `wide` is the `fWide` flag,
    /// and the names are NUL-separated and double-NUL-terminated.
    fn dropfiles(names_offset: u32, wide: u32, internal_id: u64, names: &[&str]) -> Vec<u8> {
        let mut out = vec![0u8; names_offset as usize];
        out[0..4].copy_from_slice(&names_offset.to_le_bytes());
        out[16..20].copy_from_slice(&wide.to_le_bytes());
        if names_offset == 28 {
            out[20..24].copy_from_slice(&((internal_id & 0xffff_ffff) as u32).to_le_bytes());
            out[24..28].copy_from_slice(&((internal_id >> 32) as u32).to_le_bytes());
        }
        for name in names {
            for unit in name.encode_utf16() {
                out.extend_from_slice(&unit.to_le_bytes());
            }
            out.extend_from_slice(&0u16.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn one_external_path_decodes() {
        let bytes = dropfiles(20, 1, 0, &["C:\\music\\a.mp3"]);
        assert_eq!(
            parse_dropfiles(&bytes),
            Some(vec![DragItem::FilePath {
                path: "C:\\music\\a.mp3".to_string(),
                internal_id: None
            }])
        );
    }

    #[test]
    fn a_whole_selection_decodes_in_order() {
        let bytes = dropfiles(20, 1, 0, &["C:\\a.mp3", "C:\\b.wav", "C:\\c.ogg"]);
        let items = parse_dropfiles(&bytes).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(
            items[2],
            DragItem::FilePath { path: "C:\\c.ogg".to_string(), internal_id: None }
        );
    }

    #[test]
    fn an_internal_drag_keeps_its_live_id() {
        let bytes = dropfiles(28, 1, 0x1234_5678_9abc_def0, &["tab"]);
        assert_eq!(
            parse_dropfiles(&bytes),
            Some(vec![DragItem::FilePath {
                path: "tab".to_string(),
                internal_id: Some(LiveId(0x1234_5678_9abc_def0))
            }])
        );
    }

    #[test]
    fn narrow_strings_unknown_offsets_and_stubs_are_refused() {
        assert_eq!(parse_dropfiles(&dropfiles(20, 0, 0, &["C:\\a.mp3"])), None);
        assert_eq!(parse_dropfiles(&dropfiles(24, 1, 0, &["C:\\a.mp3"])), None);
        assert_eq!(parse_dropfiles(&[0u8; 8]), None);
        assert_eq!(parse_dropfiles(&dropfiles(20, 1, 0, &[])), None);
    }

    #[test]
    fn a_surrogate_pair_survives_as_one_character() {
        let bytes = dropfiles(20, 1, 0, &["C:\\\u{1F3B5}.mp3"]);
        let items = parse_dropfiles(&bytes).unwrap();
        let DragItem::FilePath { path, .. } = &items[0] else { panic!("not a path") };
        assert_eq!(path, "C:\\\u{1F3B5}.mp3");
    }
}
