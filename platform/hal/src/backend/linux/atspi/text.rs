use super::objects::{invalid, unsupported, version, view};
use super::*;
use makepad_a11y::{mirror::text, ActionArg, Actions, Rect, TextInfo};
use makepad_dbus_wire::MethodResult;

fn info(path: &str) -> MethodResult<TextInfo> {
    view(path)?.node.text.ok_or_else(unsupported)
}
fn byte(t: &TextInfo, offset: i32, end: bool) -> MethodResult<u32> {
    if end && offset == -1 {
        return Ok(t.content.len() as u32);
    }
    if offset < 0 || offset as usize > t.content.chars().count() {
        return Err(invalid());
    }
    Ok(text::char_to_byte(&t.content, offset as u32))
}
fn range(t: &TextInfo, start: i32, end: i32) -> MethodResult<(u32, u32)> {
    let a = byte(t, start, false)?;
    let b = byte(t, end, true)?;
    if a > b {
        return Err(invalid());
    }
    Ok((a, b))
}
fn segment(t: &TextInfo, a: u32, b: u32) -> (String, i32, i32) {
    (
        t.content[a as usize..b as usize].into(),
        text::byte_to_char(&t.content, a) as i32,
        text::byte_to_char(&t.content, b) as i32,
    )
}
fn string_at(t: &TextInfo, offset: i32, granularity: u32) -> MethodResult<(String, i32, i32)> {
    let at = byte(t, offset, false)? as usize;
    let s = &t.content;
    if at == s.len() {
        return Ok((String::new(), offset, offset));
    }
    let (a, b) = match granularity {
        x if x == AtspiTextGranularity::Char as u32 => {
            (at, text::char_to_byte(s, offset as u32 + 1) as usize)
        }
        x if x == AtspiTextGranularity::Word as u32 => {
            let class = |c: char| {
                if c.is_alphanumeric() || c == '_' {
                    0
                } else if c.is_whitespace() {
                    1
                } else {
                    2
                }
            };
            let kind = class(s[at..].chars().next().unwrap());
            let a = s[..at]
                .char_indices()
                .rev()
                .find(|(_, c)| class(*c) != kind)
                .map_or(0, |(i, c)| i + c.len_utf8());
            let b = s[at..]
                .char_indices()
                .find(|(_, c)| class(*c) != kind)
                .map_or(s.len(), |(i, _)| at + i);
            (a, b)
        }
        x if x == AtspiTextGranularity::Line as u32 && !t.lines.is_empty() => {
            let (a, b) = text::line_range(t, text::line_for_byte(t, at as u32));
            (a as usize, b as usize)
        }
        x if x == AtspiTextGranularity::Line as u32
            || x == AtspiTextGranularity::Paragraph as u32 =>
        {
            (
                s[..at].rfind('\n').map_or(0, |i| i + 1),
                s[at..].find('\n').map_or(s.len(), |i| at + i + 1),
            )
        }
        x if x == AtspiTextGranularity::Sentence as u32 => {
            let stop = |c: char| matches!(c, '.' | '!' | '?' | '\n');
            (
                s[..at]
                    .char_indices()
                    .rev()
                    .find(|(_, c)| stop(*c))
                    .map_or(0, |(i, c)| i + c.len_utf8()),
                s[at..]
                    .char_indices()
                    .find(|(_, c)| stop(*c))
                    .map_or(s.len(), |(i, c)| at + i + c.len_utf8()),
            )
        }
        _ => return Err(invalid()),
    };
    Ok(segment(t, a as u32, b as u32))
}
fn boundary_granularity(boundary: u32) -> MethodResult<u32> {
    Ok(match boundary {
        0 => AtspiTextGranularity::Char,
        1 | 2 => AtspiTextGranularity::Word,
        3 | 4 => AtspiTextGranularity::Sentence,
        5 | 6 => AtspiTextGranularity::Line,
        _ => return Err(invalid()),
    } as u32)
}
impl TextServer for Objects {
    version!();
    fn prop_character_count(&mut self, path: &str) -> MethodResult<i32> {
        Ok(info(path)?.content.chars().count() as i32)
    }
    fn prop_caret_offset(&mut self, path: &str) -> MethodResult<i32> {
        let t = info(path)?;
        Ok(text::byte_to_char(&t.content, t.selection.1) as i32)
    }
    fn get_text(&mut self, path: &str, start: i32, end: i32) -> MethodResult<String> {
        let t = info(path)?;
        let (a, b) = range(&t, start, end)?;
        Ok(segment(&t, a, b).0)
    }
    fn get_string_at_offset(
        &mut self,
        path: &str,
        offset: i32,
        granularity: u32,
    ) -> MethodResult<(String, i32, i32)> {
        string_at(&info(path)?, offset, granularity)
    }
    fn get_character_at_offset(&mut self, path: &str, offset: i32) -> MethodResult<i32> {
        Ok(self
            .get_text(path, offset, offset.checked_add(1).ok_or_else(invalid)?)?
            .chars()
            .next()
            .map_or(0, |c| c as i32))
    }
    fn get_text_at_offset(
        &mut self,
        path: &str,
        offset: i32,
        boundary: u32,
    ) -> MethodResult<(String, i32, i32)> {
        self.get_string_at_offset(path, offset, boundary_granularity(boundary)?)
    }
    fn get_text_before_offset(
        &mut self,
        path: &str,
        offset: i32,
        boundary: u32,
    ) -> MethodResult<(String, i32, i32)> {
        let (_, start, _) = self.get_text_at_offset(path, offset, boundary)?;
        if start == 0 {
            return Ok((String::new(), 0, 0));
        }
        self.get_text_at_offset(path, start - 1, boundary)
    }
    fn get_text_after_offset(
        &mut self,
        path: &str,
        offset: i32,
        boundary: u32,
    ) -> MethodResult<(String, i32, i32)> {
        let (_, _, end) = self.get_text_at_offset(path, offset, boundary)?;
        self.get_text_at_offset(path, end, boundary)
    }
    fn get_character_extents(
        &mut self,
        path: &str,
        offset: i32,
        coord: u32,
    ) -> MethodResult<(i32, i32, i32, i32)> {
        self.get_range_extents(
            path,
            offset,
            offset.checked_add(1).ok_or_else(invalid)?,
            coord,
        )
    }
    fn get_range_extents(
        &mut self,
        path: &str,
        start: i32,
        end: i32,
        coord: u32,
    ) -> MethodResult<(i32, i32, i32, i32)> {
        let t = info(path)?;
        let (a, b) = range(&t, start, end)?;
        let rect = text::range_rects(&t, a, b)
            .into_iter()
            .reduce(|a, b| {
                let x = a.x.min(b.x);
                let y = a.y.min(b.y);
                Rect {
                    x,
                    y,
                    w: (a.x + a.w).max(b.x + b.w) - x,
                    h: (a.y + a.h).max(b.y + b.h) - y,
                }
            })
            .unwrap_or(view(path)?.node.bounds);
        self.rect(path, rect, coord)
    }
    fn get_offset_at_point(&mut self, path: &str, x: i32, y: i32, coord: u32) -> MethodResult<i32> {
        let (dx, dy) = self.offset(path, coord)?;
        let t = info(path)?;
        Ok(text::byte_to_char(
            &t.content,
            text::byte_at_point(&t, x as f32 - dx, y as f32 - dy),
        ) as i32)
    }
    fn get_n_selections(&mut self, path: &str) -> MethodResult<i32> {
        let t = info(path)?;
        Ok(i32::from(t.selection.0 != t.selection.1))
    }
    fn get_selection(&mut self, path: &str, index: i32) -> MethodResult<(i32, i32)> {
        let t = info(path)?;
        if index != 0 || t.selection.0 == t.selection.1 {
            return Err(invalid());
        }
        let a = text::byte_to_char(&t.content, t.selection.0) as i32;
        let b = text::byte_to_char(&t.content, t.selection.1) as i32;
        Ok((a.min(b), a.max(b)))
    }
    fn set_selection(
        &mut self,
        path: &str,
        index: i32,
        start: i32,
        end: i32,
    ) -> MethodResult<bool> {
        if index != 0 {
            return Err(invalid());
        }
        let t = info(path)?;
        let a = byte(&t, start, false)?;
        let b = byte(&t, end, false)?;
        self.enqueue(path, Actions::SET_TEXT_SELECTION, ActionArg::Range(a, b))
    }
    fn set_caret_offset(&mut self, path: &str, offset: i32) -> MethodResult<bool> {
        self.set_selection(path, 0, offset, offset)
    }
    fn add_selection(&mut self, path: &str, start: i32, end: i32) -> MethodResult<bool> {
        if self.get_n_selections(path)? != 0 {
            return Ok(false);
        }
        self.set_selection(path, 0, start, end)
    }
    fn remove_selection(&mut self, path: &str, index: i32) -> MethodResult<bool> {
        if index != 0 || self.get_n_selections(path)? == 0 {
            return Err(invalid());
        }
        let offset = self.prop_caret_offset(path)?;
        self.set_caret_offset(path, offset)
    }
    fn get_attribute_value(&mut self, path: &str, offset: i32, _: &str) -> MethodResult<String> {
        byte(&info(path)?, offset, false)?;
        Ok(String::new())
    }
    fn get_attributes(
        &mut self,
        path: &str,
        offset: i32,
    ) -> MethodResult<(Vec<(String, String)>, i32, i32)> {
        byte(&info(path)?, offset, false)?;
        Ok((vec![], 0, self.prop_character_count(path)?))
    }
    fn get_default_attributes(&mut self, _: &str) -> MethodResult<Vec<(String, String)>> {
        Ok(vec![])
    }
    fn get_default_attribute_set(&mut self, path: &str) -> MethodResult<Vec<(String, String)>> {
        self.get_default_attributes(path)
    }
    fn get_attribute_run(
        &mut self,
        path: &str,
        offset: i32,
        _: bool,
    ) -> MethodResult<(Vec<(String, String)>, i32, i32)> {
        TextServer::get_attributes(self, path, offset)
    }
    fn scroll_substring_to(
        &mut self,
        path: &str,
        start: i32,
        end: i32,
        _: u32,
    ) -> MethodResult<bool> {
        range(&info(path)?, start, end)?;
        self.enqueue(path, Actions::SCROLL_INTO_VIEW, ActionArg::None)
    }
}
impl EditableTextServer for Objects {
    version!();
    fn set_text_contents(&mut self, path: &str, contents: &str) -> MethodResult<bool> {
        let t = info(path)?;
        self.enqueue(
            path,
            Actions::REPLACE_TEXT,
            ActionArg::Replace(0, t.content.len() as u32, contents.into()),
        )
    }
    fn insert_text(
        &mut self,
        path: &str,
        position: i32,
        contents: &str,
        length: i32,
    ) -> MethodResult<bool> {
        let t = info(path)?;
        let at = byte(&t, position, false)?;
        // AT-SPI positions are code points, but InsertText.length is a UTF-8 byte count.
        if length < 0 {
            return Err(invalid());
        }
        let end = (length as usize).min(contents.len());
        if !contents.is_char_boundary(end) {
            return Err(invalid());
        }
        self.enqueue(
            path,
            Actions::REPLACE_TEXT,
            ActionArg::Replace(at, at, contents[..end].into()),
        )
    }
    fn delete_text(&mut self, path: &str, start: i32, end: i32) -> MethodResult<bool> {
        let (a, b) = range(&info(path)?, start, end)?;
        self.enqueue(
            path,
            Actions::REPLACE_TEXT,
            ActionArg::Replace(a, b, String::new()),
        )
    }
}
