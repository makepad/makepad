use crate::{document, document::Element, line, token::TokenInfo, Affinity, Document};

#[derive(Debug, PartialEq)]
pub struct Context<'a> {
    text: &'a mut Vec<String>,
    token_infos: &'a mut Vec<Vec<TokenInfo>>,
    text_inlays: &'a mut Vec<Vec<(usize, String)>>,
    line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
    wrap_bytes: &'a mut Vec<Vec<usize>>,
    fold_column: &'a mut Vec<usize>,
    scale: &'a mut Vec<f64>,
    document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
    summed_heights: &'a mut Vec<f64>,
}

impl<'a> Context<'a> {
    pub fn new(
        text: &'a mut Vec<String>,
        token_infos: &'a mut Vec<Vec<TokenInfo>>,
        text_inlays: &'a mut Vec<Vec<(usize, String)>>,
        line_widget_inlays: &'a mut Vec<Vec<((usize, Affinity), line::Widget)>>,
        wrap_bytes: &'a mut Vec<Vec<usize>>,
        fold_column: &'a mut Vec<usize>,
        scale: &'a mut Vec<f64>,
        document_widget_inlays: &'a mut Vec<((usize, Affinity), document::Widget)>,
        summed_heights: &'a mut Vec<f64>,
    ) -> Self {
        Self {
            text,
            token_infos,
            text_inlays,
            line_widget_inlays,
            wrap_bytes,
            fold_column,
            scale,
            document_widget_inlays,
            summed_heights,
        }
    }

    pub fn document(&self) -> Document<'_> {
        Document::new(
            self.text,
            self.token_infos,
            self.text_inlays,
            self.line_widget_inlays,
            self.wrap_bytes,
            self.fold_column,
            self.scale,
            self.document_widget_inlays,
            self.summed_heights,
        )
    }

    pub fn update_summed_heights(&mut self) {
        use std::mem;

        let start = self.summed_heights.len();
        let mut summed_height = if start == 0 {
            0.0
        } else {
            self.summed_heights[start - 1]
        };
        let mut summed_heights = mem::take(self.summed_heights);
        for element in self
            .document()
            .elements(start, self.document().line_count())
        {
            match element {
                Element::Line(_, line) => {
                    summed_height += line.height();
                    summed_heights.push(summed_height);
                }
                Element::Widget(_, widget) => {
                    summed_height += widget.height;
                }
            }
        }
        *self.summed_heights = summed_heights;
    }
}
