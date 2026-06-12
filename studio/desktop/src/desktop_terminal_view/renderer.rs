use crate::makepad_widgets::*;
use crate::makepad_widgets::text::geom::Point;
use crate::desktop_terminal_view::{DesktopTerminalView, CachedTerminalGlyph};
use makepad_studio_protocol::hub_protocol::TerminalFramebuffer;

impl DesktopTerminalView {
    pub(super) fn fallback_cell_metrics(&self) -> (f64, f64) {
        let w = (self.font_size * self.cell_width_factor).max(1.0);
        let h = (self.font_size * self.cell_height_factor).max(1.0);
        (w, h)
    }

    pub(super) fn refresh_cell_metrics(&mut self, cx: &mut Cx2d) {
        self.draw_text.text_style.font_size = self.font_size as f32;
        let (fallback_w, fallback_h) = self.fallback_cell_metrics();

        let layout = self
            .draw_text
            .layout(cx, 0.0, 0.0, None, false, Align::default(), "M");
        let Some(first_row) = layout.rows.first() else {
            self.cell_width = fallback_w;
            self.cell_height = fallback_h;
            self.cell_offset_y = 0.0;
            return;
        };
        let Some(first_glyph) = first_row.glyphs.first() else {
            self.cell_width = fallback_w;
            self.cell_height = fallback_h;
            self.cell_offset_y = 0.0;
            return;
        };

        let width_in_lpxs = first_glyph.advance_in_lpxs();
        let glyph_h_in_lpxs = first_glyph.ascender_in_lpxs() - first_glyph.descender_in_lpxs();
        let line_spacing_in_lpxs = glyph_h_in_lpxs * self.draw_text.text_style.line_spacing;

        self.cell_width = if width_in_lpxs > 0.0 {
            width_in_lpxs as f64
        } else {
            fallback_w
        };
        self.cell_height = if line_spacing_in_lpxs > 0.0 {
            line_spacing_in_lpxs as f64
        } else {
            fallback_h
        };
        self.cell_offset_y = ((self.cell_height - glyph_h_in_lpxs as f64) * 0.5).max(0.0);
    }

    pub(super) fn cell_metrics(&self) -> (f64, f64) {
        let (fallback_w, fallback_h) = self.fallback_cell_metrics();
        (
            if self.cell_width > 0.0 {
                self.cell_width
            } else {
                fallback_w
            },
            if self.cell_height > 0.0 {
                self.cell_height
            } else {
                fallback_h
            },
        )
    }

    pub(super) fn current_scroll_pixels(&self) -> f64 {
        self.scroll_bars.get_scroll_pos().y.max(0.0)
    }

    pub(super) fn content_height_for_total_lines(&self, total_lines: usize) -> f64 {
        let (_, cell_height) = self.cell_metrics();
        total_lines.max(1) as f64 * cell_height + self.pad_y * 2.0
    }

    pub(super) fn max_scroll_pixels_for_total_lines(&self, total_lines: usize) -> f64 {
        let content_height = self.content_height_for_total_lines(total_lines);
        if content_height <= self.viewport_rect.size.y + 0.1 {
            return 0.0;
        }
        content_height - self.viewport_rect.size.y
    }

    pub(super) fn is_scrolled_to_bottom(&self, total_lines: usize) -> bool {
        self.current_scroll_pixels() >= self.max_scroll_pixels_for_total_lines(total_lines) - 1.0
    }

    pub(super) fn clamp_scroll_position(&mut self, cx: &mut Cx, total_lines: usize) {
        let y = self
            .current_scroll_pixels()
            .min(self.max_scroll_pixels_for_total_lines(total_lines));
        let _ = self.scroll_bars.set_scroll_pos_no_clip(cx, dvec2(0.0, y));
    }

    pub(super) fn stick_to_bottom(&mut self, cx: &mut Cx, total_lines: usize) {
        let y = self.max_scroll_pixels_for_total_lines(total_lines);
        let _ = self.scroll_bars.set_scroll_pos_no_clip(cx, dvec2(0.0, y));
        self.follow_output = true;
    }

    pub(super) fn scrollbar_total_lines(frame: &TerminalFramebuffer) -> usize {
        frame.total_lines
    }

    pub(super) fn invalidate_glyph_cache_if_needed(&mut self, cx: &Cx2d) {
        let font_size = self.draw_text.text_style.font_size;
        let font_scale = self.draw_text.font_scale;
        let dpi_factor = cx.current_dpi_factor();
        if self.glyph_cache_font_size.to_bits() == font_size.to_bits()
            && self.glyph_cache_font_scale.to_bits() == font_scale.to_bits()
            && self.glyph_cache_dpi_factor.to_bits() == dpi_factor.to_bits()
        {
            return;
        }
        self.glyph_cache.clear();
        self.glyph_cache_font_size = font_size;
        self.glyph_cache_font_scale = font_scale;
        self.glyph_cache_dpi_factor = dpi_factor;
    }

    pub(super) fn cached_terminal_glyph(&mut self, cx: &mut Cx2d, ch: char) -> Option<CachedTerminalGlyph> {
        if let Some(cached) = self.glyph_cache.get(&ch) {
            return Some(*cached);
        }
        let mut utf8 = [0u8; 4];
        let text = ch.encode_utf8(&mut utf8);
        let run = self.draw_text.prepare_single_line_run(cx, text)?;
        let glyph = run.glyphs.first()?;
        let cached = CachedTerminalGlyph {
            rasterized: glyph.rasterized,
            font_size_in_lpxs: glyph.font_size_in_lpxs,
            x_offset_in_lpxs: glyph.pen_x_in_lpxs + glyph.offset_x_in_lpxs,
            baseline_offset_in_lpxs: run.ascender_in_lpxs,
        };
        self.glyph_cache.insert(ch, cached);
        Some(cached)
    }

    pub(super) fn decode_cell(
        frame: &TerminalFramebuffer,
        row: usize,
        col: usize,
    ) -> Option<(char, Vec4f, Vec4f)> {
        let cols = frame.cols as usize;
        let rows = frame.rows as usize;
        if row >= rows || col >= cols {
            return None;
        }
        let idx = (row * cols + col) * 10;
        if idx + 9 >= frame.cells.len() {
            return None;
        }
        let codepoint = u32::from_le_bytes([
            frame.cells[idx],
            frame.cells[idx + 1],
            frame.cells[idx + 2],
            frame.cells[idx + 3],
        ]);
        let ch = char::from_u32(codepoint).unwrap_or(' ');
        let fg = vec4(
            frame.cells[idx + 4] as f32 / 255.0,
            frame.cells[idx + 5] as f32 / 255.0,
            frame.cells[idx + 6] as f32 / 255.0,
            1.0,
        );
        let bg = vec4(
            frame.cells[idx + 7] as f32 / 255.0,
            frame.cells[idx + 8] as f32 / 255.0,
            frame.cells[idx + 9] as f32 / 255.0,
            1.0,
        );
        Some((if ch == '\0' { ' ' } else { ch }, fg, bg))
    }

    pub(super) fn decode_rgb(rgb: u32) -> Vec4f {
        vec4(
            ((rgb >> 16) & 0xff) as f32 / 255.0,
            ((rgb >> 8) & 0xff) as f32 / 255.0,
            (rgb & 0xff) as f32 / 255.0,
            0.20,
        )
    }

    pub(super) fn requested_frame_range(
        visible_top_row: usize,
        visible_rows: usize,
        selection: Option<((usize, usize), (usize, usize))>,
    ) -> (usize, usize) {
        let mut start_row = visible_top_row;
        let mut end_row_exclusive = visible_top_row.saturating_add(visible_rows.max(1));

        if let Some(((selection_start_row, _), (selection_end_row, _))) = selection {
            start_row = start_row.min(selection_start_row);
            end_row_exclusive = end_row_exclusive.max(selection_end_row.saturating_add(1));
        }

        let max_span = u16::MAX as usize;
        if end_row_exclusive.saturating_sub(start_row) > max_span {
            end_row_exclusive = start_row.saturating_add(max_span);
        }

        (start_row, end_row_exclusive)
    }

    pub(super) fn visible_frame_rows(
        frame: &TerminalFramebuffer,
        scroll_y: f64,
        cell_height: f64,
        max_visible_rows: usize,
        screen_top: f64,
    ) -> Option<(usize, usize, f64)> {
        let frame_rows = frame.rows as usize;
        if frame_rows == 0 || cell_height <= 0.0 {
            return None;
        }

        let frame_top_pixels = frame.top_row as f64 * cell_height;
        let scroll_delta = (scroll_y - frame_top_pixels).max(0.0);
        let start_row = ((scroll_delta / cell_height).floor() as usize).min(frame_rows);
        let render_rows = frame_rows
            .saturating_sub(start_row)
            .min(max_visible_rows.max(1));
        if render_rows == 0 {
            return None;
        }

        let intra_row_offset = scroll_delta - start_row as f64 * cell_height;
        let origin_y = screen_top - intra_row_offset;
        Some((start_row, render_rows, origin_y))
    }

    pub(super) fn draw_framebuffer(&mut self, cx: &mut Cx2d, frame: &TerminalFramebuffer) {
        let cols = frame.cols as usize;
        let rows = frame.rows as usize;
        if cols == 0 || rows == 0 {
            return;
        }

        let (cell_width, cell_height) = self.cell_metrics();

        let screen_top = self.unscrolled_rect.pos.y + self.pad_y;
        let screen_bottom = self.unscrolled_rect.pos.y + self.unscrolled_rect.size.y - self.pad_y;
        let usable_height = (screen_bottom - screen_top).max(0.0);
        let max_visible_rows = (usable_height / cell_height).ceil().max(1.0) as usize + 2;
        let scroll_y = self.current_scroll_pixels();
        let Some((start_row, render_rows, origin_y)) =
            Self::visible_frame_rows(frame, scroll_y, cell_height, max_visible_rows, screen_top)
        else {
            return;
        };

        let origin_x = self.unscrolled_rect.pos.x + self.pad_x;
        let default_bg = Self::decode_rgb(frame.default_bg_rgb);
        let has_focus = cx.has_key_focus(self.scroll_bars.area());

        self.draw_cell_bg.new_draw_call(cx);
        self.draw_cursor.new_draw_call(cx);
        self.draw_text.new_draw_call(cx);
        self.draw_text.begin_many_instances(cx);
        self.invalidate_glyph_cache_if_needed(cx);

        for i in 0..render_rows {
            let frame_row = start_row + i;
            let virtual_row = frame.top_row + frame_row;
            let y = origin_y + i as f64 * cell_height;
            for col in 0..cols {
                let Some((ch, fg_color, bg_color)) = Self::decode_cell(frame, frame_row, col)
                else {
                    continue;
                };
                let x = origin_x + col as f64 * cell_width;

                let selected = self.is_cell_selected(virtual_row, col);
                if selected {
                    self.draw_cell_bg.color = if has_focus {
                        self.selection_color_focus
                    } else {
                        self.selection_color_unfocus
                    };
                    self.draw_cell_bg.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(x, y),
                            size: dvec2(cell_width, cell_height),
                        },
                    );
                } else if bg_color != default_bg {
                    self.draw_cell_bg.color = bg_color;
                    self.draw_cell_bg.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(x, y),
                            size: dvec2(cell_width, cell_height),
                        },
                    );
                }

                if ch != ' ' {
                    if let Some(glyph) = self.cached_terminal_glyph(cx, ch) {
                        let baseline_y = y
                            + self.cell_offset_y
                            + self.text_y_offset
                            + glyph.baseline_offset_in_lpxs as f64;
                        self.draw_text.draw_rasterized_glyph_abs(
                            cx,
                            Point::new(
                                (x + glyph.x_offset_in_lpxs as f64) as f32,
                                baseline_y as f32,
                            ),
                            glyph.font_size_in_lpxs,
                            glyph.rasterized,
                            fg_color,
                        );
                    } else {
                        let mut s = [0u8; 4];
                        let text = ch.encode_utf8(&mut s);
                        self.draw_text.color = fg_color;
                        self.draw_text.draw_abs(
                            cx,
                            dvec2(x, y + self.cell_offset_y + self.text_y_offset),
                            text,
                        );
                    }
                }
            }
        }
        self.draw_text.end_many_instances(cx);

        if frame.cursor_visible && frame.cursor_row >= 0 {
            let cursor_row = frame.cursor_row as usize;
            if cursor_row >= start_row && cursor_row < start_row + render_rows {
                let visible_row = cursor_row - start_row;
                let cursor_col = (frame.cursor_col as usize).min(cols.saturating_sub(1));
                let cx_x = origin_x + cursor_col as f64 * cell_width;
                let cx_y = origin_y + visible_row as f64 * cell_height + self.cursor_y_offset;
                self.ime_pos = Some(dvec2(
                    cx_x - self.unscrolled_rect.pos.x,
                    cx_y - self.unscrolled_rect.pos.y + cell_height,
                ));
                self.draw_cursor.focus = if has_focus { 1.0 } else { 0.0 };
                self.draw_cursor.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(cx_x, cx_y),
                        size: dvec2(cell_width, cell_height),
                    },
                );
            }
        }
    }
}
