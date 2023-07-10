use {
    crate::{line, token::TokenInfo, Affinity, Line},
    std::slice,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Document<'a> {
    text: &'a Vec<String>,
    token_infos: &'a [Vec<TokenInfo>],
    text_inlays: &'a [Vec<(usize, String)>],
    line_widget_inlays: &'a [Vec<((usize, Affinity), line::Widget)>],
    wrap_bytes: &'a [Vec<usize>],
    fold_column: &'a [usize],
    scale: &'a [f64],
    widget_inlays: &'a [((usize, Affinity), Widget)],
    summed_heights: &'a [f64],
}

impl<'a> Document<'a> {
    pub fn new(
        text: &'a Vec<String>,
        token_infos: &'a [Vec<TokenInfo>],
        text_inlays: &'a [Vec<(usize, String)>],
        line_widget_inlays: &'a [Vec<((usize, Affinity), line::Widget)>],
        wrap_bytes: &'a [Vec<usize>],
        fold_column: &'a [usize],
        scale: &'a [f64],
        widget_inlays: &'a [((usize, Affinity), Widget)],
        summed_heights: &'a [f64],
    ) -> Self {
        Self {
            text,
            token_infos,
            text_inlays,
            line_widget_inlays,
            wrap_bytes,
            fold_column,
            scale,
            widget_inlays,
            summed_heights,
        }
    }

    pub fn compute_width(&self, tab_column_count: usize) -> f64 {
        let mut max_width = 0.0f64;
        for element in self.elements(0, self.line_count()) {
            max_width = max_width.max(match element {
                Element::Line(_, line) => line.compute_width(tab_column_count),
                Element::Widget(_, widget) => widget.width,
            });
        }
        max_width
    }

    pub fn height(&self) -> f64 {
        self.summed_heights[self.line_count() - 1]
    }

    pub fn find_first_line_ending_after_y(&self, y: f64) -> usize {
        match self
            .summed_heights
            .binary_search_by(|summed_height| summed_height.partial_cmp(&y).unwrap())
        {
            Ok(line_index) => line_index + 1,
            Err(line_index) => line_index,
        }
    }

    pub fn find_first_line_starting_after_y(&self, y: f64) -> usize {
        match self
            .summed_heights
            .binary_search_by(|summed_height| summed_height.partial_cmp(&y).unwrap())
        {
            Ok(line_index) => line_index + 1,
            Err(line_index) => {
                if line_index == self.line_count() {
                    line_index
                } else {
                    line_index + 1
                }
            }
        }
    }

    pub fn line_count(&self) -> usize {
        self.text.len()
    }

    pub fn line(&self, line: usize) -> Line<'a> {
        Line::new(
            &self.text[line],
            &self.token_infos[line],
            &self.text_inlays[line],
            &self.line_widget_inlays[line],
            &self.wrap_bytes[line],
            self.fold_column[line],
            self.scale[line],
        )
    }

    pub fn lines(&self, start_line: usize, end_line: usize) -> Lines<'a> {
        Lines {
            text: self.text[start_line..end_line].iter(),
            token_infos: self.token_infos[start_line..end_line].iter(),
            text_inlays: self.text_inlays[start_line..end_line].iter(),
            line_widget_inlays: self.line_widget_inlays[start_line..end_line].iter(),
            wrap_bytes: self.wrap_bytes[start_line..end_line].iter(),
            fold_column: self.fold_column[start_line..end_line].iter(),
            scale: self.scale[start_line..end_line].iter(),
        }
    }

    pub fn elements(&self, start_line: usize, end_line: usize) -> Elements<'a> {
        Elements {
            lines: self.lines(start_line, end_line),
            widget_inlays: &self.widget_inlays[self
                .widget_inlays
                .iter()
                .position(|((line, _), _)| *line >= start_line)
                .unwrap_or(self.widget_inlays.len())..],
            line: start_line,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Lines<'a> {
    text: slice::Iter<'a, String>,
    token_infos: slice::Iter<'a, Vec<TokenInfo>>,
    text_inlays: slice::Iter<'a, Vec<(usize, String)>>,
    line_widget_inlays: slice::Iter<'a, Vec<((usize, Affinity), line::Widget)>>,
    wrap_bytes: slice::Iter<'a, Vec<usize>>,
    fold_column: slice::Iter<'a, usize>,
    scale: slice::Iter<'a, f64>,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(Line::new(
            self.text.next()?,
            self.token_infos.next()?,
            self.text_inlays.next()?,
            self.line_widget_inlays.next()?,
            self.wrap_bytes.next()?,
            *self.fold_column.next()?,
            *self.scale.next()?,
        ))
    }
}

#[derive(Clone, Debug)]
pub struct Elements<'a> {
    lines: Lines<'a>,
    widget_inlays: &'a [((usize, Affinity), Widget)],
    line: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Element<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self
            .widget_inlays
            .first()
            .map_or(false, |((line, bias), _)| {
                *line == self.line && *bias == Affinity::Before
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::Before, *widget));
        }
        if self
            .widget_inlays
            .first()
            .map_or(false, |((line, bias), _)| {
                *line == self.line && *bias == Affinity::After
            })
        {
            let ((_, widget), widget_inlays) = self.widget_inlays.split_first().unwrap();
            self.widget_inlays = widget_inlays;
            return Some(Element::Widget(Affinity::After, *widget));
        }
        let line = self.lines.next()?;
        self.line += 1;
        Some(Element::Line(false, line))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Element<'a> {
    Line(bool, Line<'a>),
    Widget(Affinity, Widget),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Widget {
    pub id: usize,
    pub width: f64,
    pub height: f64,
}

impl Widget {
    pub fn new(id: usize, width: f64, height: f64) -> Self {
        Self { id, width, height }
    }
}
