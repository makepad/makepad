//! "An image is just numbers": a procedurally rendered black-and-white
//! picture (shaded spheres on a checkerboard) shown as a 512×512 grid —
//! each cell's background is the pixel and its text is the 0..255 value.
//! Zoom the cell size down to see the picture, up to read the numbers.
//! Every visible cell batches into two draw calls (quads + glyphs).

use makepad_widgets::*;

const W: usize = 512;
const H: usize = 512;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.PixelsTabBase = #(PixelsTab::register_widget(vm))
    mod.widgets.PixelsTab = set_type_default() do mod.widgets.PixelsTabBase{
        width: Fill height: Fill
        flow: Down

        toolbar := View{
            width: Fill height: 48
            flow: Right spacing: 14
            padding: Inset{left: 10, right: 10, top: 6, bottom: 6}
            align: Align{y: 0.5}

            zoom := Slider{
                width: 260
                text: "Cell size"
                min: 4.0 max: 56.0
                default: 5.0
                step: 1.0
            }
            values_check := CheckBox{text: "Show values" active: true}
            stats := Label{
                text: ""
                draw_text +: {color: #xaaaaaa text_style +: {font_size: 8.5}}
            }
        }

        grid := DataGrid{
            width: Fill height: Fill
            rows: 512
            cols: 512
            default_col_width: 5.0
            default_row_height: 5.0
            min_col_width: 4.0
            min_row_height: 4.0
            row_header_width: 44.0
            col_header_height: 20.0
            cell_pad_x: 1.0
        }
    }
}

/// Render the picture once: gradient sky, perspective checkerboard floor,
/// two lit spheres with soft shadows.
fn render_picture() -> Vec<u8> {
    let mut img = vec![0u8; W * H];
    // composed for the default zoomed-out viewport (~290×170 cells visible)
    let horizon = 128.0;
    let spheres = [(130.0f64, 82.0f64, 58.0f64), (225.0, 108.0, 24.0)];
    let light = {
        let l: (f64, f64, f64) = (-0.55, -0.65, 0.52);
        let len = (l.0 * l.0 + l.1 * l.1 + l.2 * l.2).sqrt();
        (l.0 / len, l.1 / len, l.2 / len)
    };
    for y in 0..H {
        for x in 0..W {
            let fx = x as f64;
            let fy = y as f64;
            // background
            let mut v: f64 = if fy < horizon {
                // sky gradient with a soft sun glow top-left
                let t = fy / horizon;
                let base = 232.0 - 105.0 * t;
                let dx = (fx - 40.0) / 90.0;
                let dy = (fy - 28.0) / 90.0;
                let glow = (1.0 - (dx * dx + dy * dy)).max(0.0) * 34.0;
                base + glow
            } else {
                // checkerboard floor with perspective
                let depth = (fy - horizon) / (H as f64 - horizon);
                let persp = 1.0 / (0.12 + depth);
                let u = (fx - W as f64 * 0.5) * persp * 0.02;
                let w = persp * 0.35;
                let checker = ((u.floor() as i64 + w.floor() as i64) % 2 + 2) % 2;
                let base = if checker == 0 { 165.0 } else { 95.0 };
                // fade with distance
                base * (0.55 + 0.45 * depth)
            };
            // soft shadows on the floor
            if fy >= horizon {
                for (sx, sy, r) in spheres.iter() {
                    let shadow_y = sy + r * 1.05;
                    let dx = (fx - sx) / (r * 1.35);
                    let dy = (fy - shadow_y.max(horizon + 8.0)) / (r * 0.42);
                    let d = dx * dx + dy * dy;
                    if d < 1.0 {
                        v *= 0.45 + 0.55 * d;
                    }
                }
            }
            // spheres
            for (sx, sy, r) in spheres.iter() {
                let dx = fx - sx;
                let dy = fy - sy;
                let d2 = dx * dx + dy * dy;
                if d2 <= r * r {
                    let nz = (r * r - d2).sqrt() / r;
                    let nx = dx / r;
                    let ny = dy / r;
                    let lambert = (nx * light.0 + ny * light.1 + nz * light.2).max(0.0);
                    // specular
                    let refl = 2.0 * lambert;
                    let rx = refl * nx - light.0;
                    let ry = refl * ny - light.1;
                    let rz = refl * nz - light.2;
                    let spec = (rz / (rx * rx + ry * ry + rz * rz).sqrt()).max(0.0).powi(24);
                    v = 22.0 + 185.0 * lambert + 120.0 * spec;
                }
            }
            img[y * W + x] = v.clamp(0.0, 255.0) as u8;
        }
    }
    img
}

fn value_strings() -> Vec<String> {
    (0..256).map(|v| v.to_string()).collect()
}

#[derive(Script, ScriptHook, Widget)]
pub struct PixelsTab {
    #[deref]
    view: View,
    #[rust]
    picture: Option<Vec<u8>>,
    #[rust(value_strings())]
    value_strings: Vec<String>,
    #[rust(true)]
    show_values: bool,
    #[rust]
    initialized: bool,
}

impl PixelsTab {
    fn update_stats(&mut self, cx: &mut Cx) {
        let grid = self.view.data_grid(cx, ids!(grid));
        let (vr, vc) = grid.visible_counts();
        self.view.label(cx, ids!(stats)).set_text(
            cx,
            &format!(
                "512 × 512 = 262,144 pixel cells · drawing {} × {} = {} cells in 2 batched draw calls",
                vr,
                vc,
                vr * vc
            ),
        );
    }
}

impl Widget for PixelsTab {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.picture.is_none() {
            self.picture = Some(render_picture());
        }
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut grid) = step.as_data_grid().borrow_mut() {
                if !self.initialized {
                    self.initialized = true;
                    grid.set_col_labels((0..W).map(|c| c.to_string()).collect());
                }
                grid.set_grid_size(H, W);
                let picture = self.picture.as_ref().unwrap();
                let show_values = self.show_values;
                while let Some(cell) = grid.next_cell(cx) {
                    let v = picture[cell.row * W + cell.col];
                    let g = v as f32 / 255.0;
                    let bg = vec4(g, g, g, 1.0);
                    if show_values && cell.rect.size.x >= 24.0 && cell.rect.size.y >= 12.0 {
                        let tc = if v < 118 {
                            vec4(1.0, 1.0, 1.0, 0.85)
                        } else {
                            vec4(0.0, 0.0, 0.0, 0.8)
                        };
                        let text = &self.value_strings[v as usize];
                        grid.cell_text_styled(
                            cx,
                            &cell,
                            text,
                            CellStyle {
                                bg: Some(bg),
                                color: Some(tc),
                                align: 0.5,
                                bold: false,
                                font_scale: (cell.rect.size.x / 34.0).clamp(0.7, 1.1),
                            },
                        );
                    } else {
                        grid.cell_bg(cx, &cell, bg);
                    }
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);

        let Event::Actions(actions) = event else {
            return;
        };
        if let Some(size) = self.view.slider(cx, ids!(zoom)).slided(actions) {
            let grid = self.view.data_grid(cx, ids!(grid));
            grid.set_default_sizes(cx, size.max(4.0), size.max(4.0));
            self.update_stats(cx);
        }
        if let Some(active) = self.view.check_box(cx, ids!(values_check)).changed(actions) {
            self.show_values = active;
            self.view.data_grid(cx, ids!(grid)).redraw(cx);
        }
        let grid = self.view.data_grid(cx, ids!(grid));
        for action in grid.actions(actions) {
            if let DataGridAction::Scrolled = action {
                self.update_stats(cx);
            }
        }
    }
}
