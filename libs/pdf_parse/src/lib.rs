pub mod content;
pub mod document;
pub mod filter;
pub mod font;
pub mod image;
pub mod lexer;
pub mod object;
pub mod page;
pub mod parser;

#[cfg(test)]
mod tests;

pub use content::{parse_content_stream, PdfOp, TextArrayItem};
pub use document::PdfDocument;
pub use font::{char_width, decode_text};
pub use image::{decode_inline_image, extract_image, PdfImage};
pub use lexer::{PdfError, PdfResult};
pub use object::*;
pub use page::PdfPage;

/// Generate a rich PDF document for testing.
/// Creates varied pages: title page, data tables, charts, vector art, multi-column text, code listings.
pub fn generate_test_pdf(num_pages: usize) -> Vec<u8> {
    let mut pdf = PdfWriter::new();
    let total = num_pages.max(1);

    for page_idx in 0..total {
        let mut c = String::new();
        // White background
        c.push_str("1 g\n0 0 612 792 re f\n");

        match page_idx % 6 {
            0 => build_title_page(&mut c, page_idx),
            1 => build_table_page(&mut c, page_idx),
            2 => build_bar_chart_page(&mut c, page_idx),
            3 => build_shapes_page(&mut c, page_idx),
            4 => build_two_column_page(&mut c, page_idx),
            5 => build_code_listing_page(&mut c, page_idx),
            _ => {}
        }

        // Footer on every page
        c.push_str("0.6 g\n");
        c.push_str("72 30 m 540 30 l S\n");
        c.push_str("BT /F1 8 Tf 0.5 0.5 0.5 rg\n");
        text_at(
            &mut c,
            72.0,
            18.0,
            &format!("Page {} of {}", page_idx + 1, total),
        );
        c.push_str("ET\n");

        pdf.add_page(612.0, 792.0, &c);
    }
    pdf.finish()
}

/// Helper: emit text at absolute position using Tm operator
fn text_at(c: &mut String, x: f64, y: f64, text: &str) {
    c.push_str(&format!("1 0 0 1 {:.1} {:.1} Tm ({}) Tj\n", x, y, text));
}

fn build_title_page(c: &mut String, idx: usize) {
    // Decorative top band
    c.push_str("0.15 0.30 0.55 rg\n0 742 612 50 re f\n");
    // Title in white on band
    c.push_str("BT /F2 28 Tf 1 1 1 rg\n");
    text_at(c, 72.0, 755.0, "Makepad PDF Viewer Demo");
    c.push_str("ET\n");
    // Subtitle
    c.push_str("BT /F1 14 Tf 0.2 0.2 0.2 rg\n");
    text_at(
        c,
        72.0,
        710.0,
        &format!(
            "Document Section {} \\267 Generated Test Document",
            idx / 6 + 1
        ),
    );
    c.push_str("ET\n");
    // Separator line
    c.push_str("0.15 0.30 0.55 RG\n2 w\n72 698 m 540 698 l S\n");
    // Description paragraphs
    let paragraphs = [
        "This is a comprehensive test PDF designed to exercise the Makepad PDF viewer.",
        "It contains tables, bar charts, geometric shapes, multi-column layouts,",
        "code listings, and various typographic styles including bold and monospace text.",
        "",
        "The purpose of this document is to verify correct rendering of:",
        "  \\267  Text positioning and font selection \\(regular, bold, monospace\\)",
        "  \\267  Vector graphics: lines, curves, filled shapes, and stroked paths",
        "  \\267  Table layouts with alternating row colors and grid lines",
        "  \\267  Complex path geometry including circles and rounded rectangles",
        "  \\267  Multi-page scrolling with the virtual viewport PortalList",
    ];
    c.push_str("BT /F1 11 Tf 0.15 0.15 0.15 rg\n");
    for (i, line) in paragraphs.iter().enumerate() {
        let y = 670.0 - i as f64 * 18.0;
        text_at(c, 72.0, y, line);
    }
    c.push_str("ET\n");
    // Feature boxes at the bottom
    let labels = ["Tables", "Charts", "Shapes", "Columns", "Code"];
    let colors: [(f64, f64, f64); 5] = [
        (0.20, 0.60, 0.86),
        (0.18, 0.80, 0.44),
        (0.91, 0.30, 0.24),
        (0.61, 0.35, 0.71),
        (0.95, 0.61, 0.07),
    ];
    for (i, (label, &(r, g, b))) in labels.iter().zip(colors.iter()).enumerate() {
        let x = 72.0 + i as f64 * 96.0;
        // Rounded-ish box (just a rect with fill)
        c.push_str(&format!("{:.2} {:.2} {:.2} rg\n", r, g, b));
        c.push_str(&format!("{:.0} 420 80 50 re f\n", x));
        // Label
        c.push_str("BT /F2 11 Tf 1 1 1 rg\n");
        text_at(c, x + 10.0, 440.0, label);
        c.push_str("ET\n");
    }
    // Decorative curves
    c.push_str("q\n");
    c.push_str("0.15 0.30 0.55 RG\n1.5 w\n");
    c.push_str("72 380 m 200 350 400 410 540 380 c S\n");
    c.push_str("0.18 0.80 0.44 RG\n");
    c.push_str("72 360 m 200 330 400 390 540 360 c S\n");
    c.push_str("0.91 0.30 0.24 RG\n");
    c.push_str("72 340 m 200 310 400 370 540 340 c S\n");
    c.push_str("Q\n");
    // Bottom decorative band
    c.push_str("0.15 0.30 0.55 rg\n0 50 612 8 re f\n");
}

fn build_table_page(c: &mut String, idx: usize) {
    // Header
    c.push_str("BT /F2 20 Tf 0.15 0.15 0.15 rg\n");
    text_at(c, 72.0, 740.0, "Data Table");
    c.push_str("ET\n");
    c.push_str("0.3 0.3 0.3 RG\n0.5 w\n72 730 m 540 730 l S\n");
    // Table: 5 columns, 10 data rows
    let col_x = [72.0, 162.0, 252.0, 372.0, 472.0];
    let headers = ["ID", "Name", "Category", "Value", "Status"];
    let row_h = 22.0;
    let table_top = 710.0;
    // Header row background
    c.push_str("0.15 0.30 0.55 rg\n");
    c.push_str(&format!(
        "72 {:.0} 468 {:.0} re f\n",
        table_top - row_h + 4.0,
        row_h
    ));
    // Header text
    c.push_str("BT /F2 10 Tf 1 1 1 rg\n");
    for (i, h) in headers.iter().enumerate() {
        text_at(c, col_x[i] + 4.0, table_top - 10.0, h);
    }
    c.push_str("ET\n");
    // Data rows
    let names = [
        "Alpha", "Beta", "Gamma", "Delta", "Epsilon", "Zeta", "Eta", "Theta", "Iota", "Kappa",
    ];
    let categories = ["Engineering", "Marketing", "Research", "Sales", "Design"];
    let statuses = ["Active", "Pending", "Complete", "Review"];
    for row in 0..10 {
        let y = table_top - (row + 1) as f64 * row_h;
        // Alternating row color
        if row % 2 == 0 {
            c.push_str("0.94 0.95 0.97 rg\n");
        } else {
            c.push_str("1 1 1 rg\n");
        }
        c.push_str(&format!(
            "72 {:.0} 468 {:.0} re f\n",
            y - row_h + 4.0,
            row_h
        ));
        // Row text
        c.push_str("BT /F1 9 Tf 0.2 0.2 0.2 rg\n");
        let base = idx * 10;
        let vals = [
            format!("{:04}", base + row + 1),
            names[row % names.len()].to_string(),
            categories[row % categories.len()].to_string(),
            format!("${},{}00", 100 + row * 37 + idx * 13, row * 5),
            statuses[row % statuses.len()].to_string(),
        ];
        for (i, v) in vals.iter().enumerate() {
            text_at(c, col_x[i] + 4.0, y - 10.0, &v);
        }
        c.push_str("ET\n");
    }
    // Grid lines (header + 10 data rows = 11 rows total)
    c.push_str("0.7 0.7 0.7 RG\n0.5 w\n");
    for row in 0..=11 {
        let y = table_top - row as f64 * row_h + 4.0;
        c.push_str(&format!("72 {:.0} m 540 {:.0} l S\n", y, y));
    }
    for &x in &[72.0, 162.0, 252.0, 372.0, 472.0, 540.0] {
        c.push_str(&format!(
            "{:.0} {:.0} m {:.0} {:.0} l S\n",
            x,
            table_top + 4.0,
            x,
            table_top - 11.0 * row_h + 4.0
        ));
    }

    // Summary box below table (11 rows = header + 10 data)
    let box_top = table_top - 11.0 * row_h - 16.0;
    c.push_str("0.94 0.97 0.94 rg\n");
    c.push_str(&format!("72 {:.0} 468 60 re f\n", box_top));
    c.push_str("0.18 0.55 0.34 RG\n1 w\n");
    c.push_str(&format!("72 {:.0} 468 60 re S\n", box_top));
    c.push_str("BT /F2 11 Tf 0.18 0.55 0.34 rg\n");
    text_at(c, 82.0, box_top + 40.0, "Summary");
    c.push_str("ET\n");
    c.push_str("BT /F1 9 Tf 0.2 0.2 0.2 rg\n");
    text_at(
        c,
        82.0,
        box_top + 18.0,
        "Total records: 10  |  Active: 3  |  Pending: 3  |  Complete: 2  |  Review: 2",
    );
    c.push_str("ET\n");
}

fn build_bar_chart_page(c: &mut String, _idx: usize) {
    // Header
    c.push_str("BT /F2 20 Tf 0.15 0.15 0.15 rg\n");
    text_at(c, 72.0, 740.0, "Performance Metrics");
    c.push_str("ET\n");
    c.push_str("0.3 0.3 0.3 RG\n0.5 w\n72 730 m 540 730 l S\n");

    // Bar chart
    let chart_left = 100.0;
    let chart_bottom = 420.0;
    let chart_w = 400.0;
    let chart_h = 280.0;
    // Axes
    c.push_str("0.3 0.3 0.3 RG\n1.5 w\n");
    c.push_str(&format!(
        "{:.0} {:.0} m {:.0} {:.0} l S\n",
        chart_left,
        chart_bottom,
        chart_left,
        chart_bottom + chart_h
    ));
    c.push_str(&format!(
        "{:.0} {:.0} m {:.0} {:.0} l S\n",
        chart_left,
        chart_bottom,
        chart_left + chart_w,
        chart_bottom
    ));
    // Y-axis grid lines and labels
    c.push_str("0.85 0.85 0.85 RG\n0.5 w\n");
    for i in 1..=5 {
        let y = chart_bottom + (i as f64 / 5.0) * chart_h;
        c.push_str(&format!(
            "{:.0} {:.0} m {:.0} {:.0} l S\n",
            chart_left,
            y,
            chart_left + chart_w,
            y
        ));
        c.push_str("BT /F1 8 Tf 0.4 0.4 0.4 rg\n");
        text_at(c, chart_left - 25.0, y - 3.0, &format!("{}", i * 20));
        c.push_str("ET\n");
    }
    // Bars
    let data = [65.0, 85.0, 45.0, 92.0, 58.0, 73.0, 88.0, 40.0];
    let labels = [
        "Q1'24", "Q2'24", "Q3'24", "Q4'24", "Q1'25", "Q2'25", "Q3'25", "Q4'25",
    ];
    let bar_colors: [(f64, f64, f64); 8] = [
        (0.20, 0.60, 0.86),
        (0.20, 0.60, 0.86),
        (0.20, 0.60, 0.86),
        (0.20, 0.60, 0.86),
        (0.18, 0.80, 0.44),
        (0.18, 0.80, 0.44),
        (0.18, 0.80, 0.44),
        (0.18, 0.80, 0.44),
    ];
    let bar_w = chart_w / data.len() as f64 * 0.7;
    let gap = chart_w / data.len() as f64 * 0.3;
    for (i, (&val, &(r, g, b))) in data.iter().zip(bar_colors.iter()).enumerate() {
        let x = chart_left + i as f64 * (bar_w + gap) + gap / 2.0;
        let h = (val / 100.0) * chart_h;
        c.push_str(&format!("{:.2} {:.2} {:.2} rg\n", r, g, b));
        c.push_str(&format!(
            "{:.1} {:.1} {:.1} {:.1} re f\n",
            x, chart_bottom, bar_w, h
        ));
        // Value label on top
        c.push_str("BT /F2 8 Tf 0.2 0.2 0.2 rg\n");
        text_at(
            c,
            x + bar_w / 4.0,
            chart_bottom + h + 5.0,
            &format!("{:.0}", val),
        );
        c.push_str("ET\n");
        // X-axis label
        c.push_str("BT /F1 7 Tf 0.4 0.4 0.4 rg\n");
        text_at(c, x, chart_bottom - 15.0, labels[i]);
        c.push_str("ET\n");
    }
    // Legend
    c.push_str("BT /F2 10 Tf 0.3 0.3 0.3 rg\n");
    text_at(c, 100.0, 390.0, "Legend:");
    c.push_str("ET\n");
    c.push_str("0.20 0.60 0.86 rg\n170 388 12 12 re f\n");
    c.push_str("BT /F1 9 Tf 0.3 0.3 0.3 rg\n");
    text_at(c, 186.0, 390.0, "2024");
    c.push_str("ET\n");
    c.push_str("0.18 0.80 0.44 rg\n230 388 12 12 re f\n");
    c.push_str("BT /F1 9 Tf 0.3 0.3 0.3 rg\n");
    text_at(c, 246.0, 390.0, "2025");
    c.push_str("ET\n");

    // Pie chart below
    c.push_str("BT /F2 16 Tf 0.15 0.15 0.15 rg\n");
    text_at(c, 72.0, 360.0, "Resource Allocation");
    c.push_str("ET\n");
    let cx_pie = 200.0;
    let cy_pie = 260.0;
    let r_pie = 80.0;
    let slices: &[(f64, f64, f64, f64, &str)] = &[
        (0.35, 0.20, 0.60, 0.86, "Engineering 35%"),
        (0.25, 0.18, 0.80, 0.44, "Product 25%"),
        (0.20, 0.91, 0.30, 0.24, "Marketing 20%"),
        (0.12, 0.61, 0.35, 0.71, "Sales 12%"),
        (0.08, 0.95, 0.61, 0.07, "Other 8%"),
    ];
    let mut angle = 0.0f64;
    for &(frac, r, g, b, _label) in slices {
        let end_angle = angle + frac * 2.0 * std::f64::consts::PI;
        c.push_str(&format!("{:.2} {:.2} {:.2} rg\n", r, g, b));
        c.push_str(&format!("{:.1} {:.1} m\n", cx_pie, cy_pie));
        let steps = 20;
        for s in 0..=steps {
            let a = angle + (end_angle - angle) * s as f64 / steps as f64;
            let px = cx_pie + r_pie * a.cos();
            let py = cy_pie + r_pie * a.sin();
            c.push_str(&format!("{:.1} {:.1} l\n", px, py));
        }
        c.push_str("h f\n");
        angle = end_angle;
    }
    // Pie outline
    c.push_str("0.3 0.3 0.3 RG\n0.5 w\n");
    angle = 0.0;
    for &(frac, _, _, _, _) in slices {
        let end_angle = angle + frac * 2.0 * std::f64::consts::PI;
        c.push_str(&format!(
            "{:.1} {:.1} m {:.1} {:.1} l S\n",
            cx_pie,
            cy_pie,
            cx_pie + r_pie * end_angle.cos(),
            cy_pie + r_pie * end_angle.sin()
        ));
        angle = end_angle;
    }
    // Pie legend
    let mut ly = 320.0;
    for &(_, r, g, b, label) in slices {
        c.push_str(&format!("{:.2} {:.2} {:.2} rg\n", r, g, b));
        c.push_str(&format!("340 {:.0} 10 10 re f\n", ly));
        c.push_str("BT /F1 9 Tf 0.2 0.2 0.2 rg\n");
        text_at(c, 356.0, ly + 1.0, label);
        c.push_str("ET\n");
        ly -= 18.0;
    }
}

fn build_shapes_page(c: &mut String, _idx: usize) {
    c.push_str("BT /F2 20 Tf 0.15 0.15 0.15 rg\n");
    text_at(c, 72.0, 740.0, "Vector Graphics Gallery");
    c.push_str("ET\n");
    c.push_str("0.3 0.3 0.3 RG\n0.5 w\n72 730 m 540 730 l S\n");

    // Row 1: Basic shapes
    c.push_str("BT /F2 12 Tf 0.3 0.3 0.3 rg\n");
    text_at(c, 72.0, 700.0, "Basic Shapes");
    c.push_str("ET\n");
    // Filled circle (approximated with bezier arcs)
    let draw_circle = |c: &mut String, cx: f64, cy: f64, r: f64| {
        let k = 0.5522847498; // magic number for cubic bezier circle approximation
        let kr = k * r;
        c.push_str(&format!("{:.1} {:.1} m\n", cx + r, cy));
        c.push_str(&format!(
            "{:.1} {:.1} {:.1} {:.1} {:.1} {:.1} c\n",
            cx + r,
            cy + kr,
            cx + kr,
            cy + r,
            cx,
            cy + r
        ));
        c.push_str(&format!(
            "{:.1} {:.1} {:.1} {:.1} {:.1} {:.1} c\n",
            cx - kr,
            cy + r,
            cx - r,
            cy + kr,
            cx - r,
            cy
        ));
        c.push_str(&format!(
            "{:.1} {:.1} {:.1} {:.1} {:.1} {:.1} c\n",
            cx - r,
            cy - kr,
            cx - kr,
            cy - r,
            cx,
            cy - r
        ));
        c.push_str(&format!(
            "{:.1} {:.1} {:.1} {:.1} {:.1} {:.1} c\n",
            cx + kr,
            cy - r,
            cx + r,
            cy - kr,
            cx + r,
            cy
        ));
        c.push_str("h\n");
    };
    // Red circle
    c.push_str("0.91 0.30 0.24 rg\n");
    draw_circle(c, 130.0, 640.0, 40.0);
    c.push_str("f\n");
    c.push_str("BT /F1 8 Tf 0.4 0.4 0.4 rg\n");
    text_at(c, 110.0, 590.0, "Circle");
    c.push_str("ET\n");

    // Blue rounded rectangle
    c.push_str("0.20 0.60 0.86 rg\n");
    let (rx, ry, rw, rh, rr) = (220.0, 610.0, 100.0, 60.0, 10.0);
    c.push_str(&format!("{:.0} {:.0} m\n", rx + rr, ry));
    c.push_str(&format!("{:.0} {:.0} l\n", rx + rw - rr, ry));
    c.push_str(&format!(
        "{:.0} {:.0} {:.0} {:.0} {:.0} {:.0} c\n",
        rx + rw,
        ry,
        rx + rw,
        ry,
        rx + rw,
        ry + rr
    ));
    c.push_str(&format!("{:.0} {:.0} l\n", rx + rw, ry + rh - rr));
    c.push_str(&format!(
        "{:.0} {:.0} {:.0} {:.0} {:.0} {:.0} c\n",
        rx + rw,
        ry + rh,
        rx + rw,
        ry + rh,
        rx + rw - rr,
        ry + rh
    ));
    c.push_str(&format!("{:.0} {:.0} l\n", rx + rr, ry + rh));
    c.push_str(&format!(
        "{:.0} {:.0} {:.0} {:.0} {:.0} {:.0} c\n",
        rx,
        ry + rh,
        rx,
        ry + rh,
        rx,
        ry + rh - rr
    ));
    c.push_str(&format!("{:.0} {:.0} l\n", rx, ry + rr));
    c.push_str(&format!(
        "{:.0} {:.0} {:.0} {:.0} {:.0} {:.0} c\n",
        rx,
        ry,
        rx,
        ry,
        rx + rr,
        ry
    ));
    c.push_str("h f\n");
    c.push_str("BT /F1 8 Tf 0.4 0.4 0.4 rg\n");
    text_at(c, 230.0, 590.0, "Rounded Rect");
    c.push_str("ET\n");

    // Green diamond
    c.push_str("0.18 0.80 0.44 rg\n");
    c.push_str("420 640 m 460 680 l 500 640 l 460 600 l h f\n");
    c.push_str("BT /F1 8 Tf 0.4 0.4 0.4 rg\n");
    text_at(c, 440.0, 590.0, "Diamond");
    c.push_str("ET\n");

    // Row 2: Stroked shapes with varying line widths
    c.push_str("BT /F2 12 Tf 0.3 0.3 0.3 rg\n");
    text_at(c, 72.0, 570.0, "Stroke Styles");
    c.push_str("ET\n");
    let line_widths = [0.5, 1.0, 2.0, 3.0, 5.0];
    for (i, &lw) in line_widths.iter().enumerate() {
        let y = 540.0 - i as f64 * 22.0;
        c.push_str(&format!("{:.1} w\n", lw));
        c.push_str("0.3 0.3 0.3 RG\n");
        c.push_str(&format!("100 {:.0} m 300 {:.0} l S\n", y, y));
        c.push_str("BT /F1 8 Tf 0.4 0.4 0.4 rg\n");
        text_at(c, 310.0, y - 3.0, &format!("{:.1}pt", lw));
        c.push_str("ET\n");
    }

    // Row 3: Concentric circles
    c.push_str("BT /F2 12 Tf 0.3 0.3 0.3 rg\n");
    text_at(c, 72.0, 420.0, "Concentric Rings");
    c.push_str("ET\n");
    let ring_colors: [(f64, f64, f64); 5] = [
        (0.91, 0.30, 0.24),
        (0.95, 0.61, 0.07),
        (0.18, 0.80, 0.44),
        (0.20, 0.60, 0.86),
        (0.61, 0.35, 0.71),
    ];
    for (i, &(r, g, b)) in ring_colors.iter().enumerate() {
        let radius = 80.0 - i as f64 * 14.0;
        c.push_str(&format!("{:.2} {:.2} {:.2} RG\n2.5 w\n", r, g, b));
        draw_circle(c, 200.0, 330.0, radius);
        c.push_str("S\n");
    }

    // Star shape on the right
    c.push_str("BT /F2 12 Tf 0.3 0.3 0.3 rg\n");
    text_at(c, 360.0, 420.0, "Star");
    c.push_str("ET\n");
    c.push_str("0.95 0.61 0.07 rg\n");
    let star_cx = 430.0;
    let star_cy = 330.0;
    let outer = 60.0;
    let inner = 25.0;
    for i in 0..10 {
        let a = std::f64::consts::PI / 2.0 + i as f64 * std::f64::consts::PI / 5.0;
        let r = if i % 2 == 0 { outer } else { inner };
        let px = star_cx + r * a.cos();
        let py = star_cy + r * a.sin();
        if i == 0 {
            c.push_str(&format!("{:.1} {:.1} m\n", px, py));
        } else {
            c.push_str(&format!("{:.1} {:.1} l\n", px, py));
        }
    }
    c.push_str("h f\n");
    // Star outline
    c.push_str("0.7 0.4 0.0 RG\n1.5 w\n");
    for i in 0..10 {
        let a = std::f64::consts::PI / 2.0 + i as f64 * std::f64::consts::PI / 5.0;
        let r = if i % 2 == 0 { outer } else { inner };
        let px = star_cx + r * a.cos();
        let py = star_cy + r * a.sin();
        if i == 0 {
            c.push_str(&format!("{:.1} {:.1} m\n", px, py));
        } else {
            c.push_str(&format!("{:.1} {:.1} l\n", px, py));
        }
    }
    c.push_str("h S\n");

    // Gradient-like rectangles
    c.push_str("BT /F2 12 Tf 0.3 0.3 0.3 rg\n");
    text_at(c, 72.0, 230.0, "Color Gradient \\(simulated\\)");
    c.push_str("ET\n");
    for i in 0..40 {
        let t = i as f64 / 39.0;
        let r = 0.2 + t * 0.7;
        let g = 0.6 - t * 0.4;
        let b = 0.9 - t * 0.6;
        c.push_str(&format!("{:.3} {:.3} {:.3} rg\n", r, g, b));
        c.push_str(&format!("{:.1} 180 11.7 30 re f\n", 72.0 + i as f64 * 11.7));
    }

    // Bezier art
    c.push_str("BT /F2 12 Tf 0.3 0.3 0.3 rg\n");
    text_at(c, 72.0, 160.0, "Bezier Curves");
    c.push_str("ET\n");
    let wave_colors = [
        (0.91, 0.30, 0.24),
        (0.18, 0.80, 0.44),
        (0.20, 0.60, 0.86),
        (0.61, 0.35, 0.71),
    ];
    for (i, &(r, g, b)) in wave_colors.iter().enumerate() {
        let base_y = 120.0 - i as f64 * 20.0;
        c.push_str(&format!("{:.2} {:.2} {:.2} RG\n2 w\n", r, g, b));
        c.push_str(&format!(
            "72 {:.0} m 200 {:.0} 400 {:.0} 540 {:.0} c S\n",
            base_y,
            base_y + 30.0,
            base_y - 30.0,
            base_y
        ));
    }
}

fn build_two_column_page(c: &mut String, idx: usize) {
    c.push_str("BT /F2 20 Tf 0.15 0.15 0.15 rg\n");
    text_at(c, 72.0, 740.0, "Technical Overview");
    c.push_str("ET\n");
    c.push_str("0.3 0.3 0.3 RG\n0.5 w\n72 730 m 540 730 l S\n");

    let col1_x = 72.0;
    let col2_x = 316.0;

    // Column separator
    c.push_str("0.85 0.85 0.85 RG\n0.5 w\n306 710 m 306 100 l S\n");

    // Left column header
    c.push_str("BT /F2 13 Tf 0.15 0.30 0.55 rg\n");
    text_at(c, col1_x, 705.0, "Architecture");
    c.push_str("ET\n");
    let left_text = [
        "The rendering pipeline uses a",
        "virtual viewport approach similar",
        "to PortalList. Only pages visible",
        "in the current scroll position",
        "are rendered to the GPU.",
        "",
        "Each page is parsed into a list",
        "of drawing operations: path",
        "construction, text placement,",
        "color changes, and coordinate",
        "transforms. These ops are then",
        "replayed through DrawVector",
        "and DrawText primitives.",
        "",
        "Font mapping uses three styles:",
        "regular, bold, and monospace.",
        "The PDF font name is matched",
        "to the appropriate DrawText",
        "instance at render time.",
    ];
    c.push_str("BT /F1 9 Tf 0.2 0.2 0.2 rg\n");
    for (i, line) in left_text.iter().enumerate() {
        let y = 688.0 - i as f64 * 14.0;
        text_at(c, col1_x, y, line);
    }
    c.push_str("ET\n");

    // Right column header
    c.push_str("BT /F2 13 Tf 0.15 0.30 0.55 rg\n");
    text_at(c, col2_x, 705.0, "Performance");
    c.push_str("ET\n");
    let right_text = [
        "Key optimizations include:",
        "",
        "1. Page-level virtualization:",
        "   only visible pages draw",
        "",
        "2. DrawList2d per page: each",
        "   page has its own draw list",
        "   via new_batch on View",
        "",
        "3. Cached content streams:",
        "   parsed once, replayed each",
        "   frame from PdfOp vectors",
        "",
        "4. Two-pass rendering:",
        "   all vector geometry first,",
        "   then all text, avoiding",
        "   DrawVector begin/end issues",
        "",
        "5. Shared page cache via Rc",
    ];
    c.push_str("BT /F1 9 Tf 0.2 0.2 0.2 rg\n");
    for (i, line) in right_text.iter().enumerate() {
        let y = 688.0 - i as f64 * 14.0;
        text_at(c, col2_x, y, line);
    }
    c.push_str("ET\n");

    // Bottom callout box
    c.push_str("0.94 0.95 0.98 rg\n72 120 468 100 re f\n");
    c.push_str("0.15 0.30 0.55 RG\n1.5 w\n72 120 468 100 re S\n");
    c.push_str("BT /F2 12 Tf 0.15 0.30 0.55 rg\n");
    text_at(c, 86.0, 200.0, "Key Insight");
    c.push_str("ET\n");
    c.push_str("BT /F1 10 Tf 0.2 0.2 0.2 rg\n");
    text_at(
        c,
        86.0,
        182.0,
        &format!(
            "Section {}: The combination of PortalList virtual scrolling and per-page DrawList2d",
            idx / 6 + 1
        ),
    );
    text_at(
        c,
        86.0,
        168.0,
        "ensures smooth scrolling even with hundreds of pages, as only 2-3 pages",
    );
    text_at(c, 86.0, 154.0, "are ever in the draw pipeline at once.");
    c.push_str("ET\n");
}

fn build_code_listing_page(c: &mut String, _idx: usize) {
    c.push_str("BT /F2 20 Tf 0.15 0.15 0.15 rg\n");
    text_at(c, 72.0, 740.0, "Code Listing");
    c.push_str("ET\n");
    c.push_str("0.3 0.3 0.3 RG\n0.5 w\n72 730 m 540 730 l S\n");

    // Code block background
    c.push_str("0.96 0.97 0.98 rg\n72 420 468 300 re f\n");
    c.push_str("0.80 0.82 0.85 RG\n0.5 w\n72 420 468 300 re S\n");

    // Code header
    c.push_str("0.90 0.91 0.93 rg\n72 696 468 24 re f\n");
    c.push_str("BT /F2 10 Tf 0.4 0.4 0.4 rg\n");
    text_at(c, 82.0, 703.0, "pdf_view.rs \\(excerpt\\)");
    c.push_str("ET\n");

    // Code lines in monospace font
    let code_lines: &[(&str, &str, &str, &str, &str)] = &[
        ("  ", "impl", " ", "PdfPageView", " {"),
        (
            "    ",
            "fn",
            " render_page\\(",
            "&mut self",
            ", cx: &mut Cx2d,",
        ),
        ("      ", "", "cached: &CachedPage, zoom: f64", "", "\\) {"),
        (
            "      ",
            "let",
            " rect = cx.turtle\\(\\).rect\\(\\);",
            "",
            "",
        ),
        ("      ", "let", " origin_x = rect.pos.x ", "as", " f32;"),
        ("      ", "let", " origin_y = rect.pos.y ", "as", " f32;"),
        ("", "", "", "", ""),
        ("      ", "// Pass 1: vector geometry", "", "", ""),
        (
            "      ",
            "self",
            ".render_vectors\\(cx, &cached.ops,",
            "",
            "",
        ),
        ("        ", "", "&cached.page, origin_x, origin_y,", "", ""),
        ("        ", "", "zoom, page_height\\);", "", ""),
        ("", "", "", "", ""),
        ("      ", "// Pass 2: text content", "", "", ""),
        ("      ", "self", ".render_text\\(cx, &cached.ops,", "", ""),
        ("        ", "", "&cached.page, origin_x, origin_y,", "", ""),
        ("        ", "", "zoom, page_height\\);", "", ""),
        ("    ", "}", "", "", ""),
        ("  ", "}", "", "", ""),
    ];
    c.push_str("BT /F3 9 Tf\n");
    for (i, (indent, kw, code, kw2, rest)) in code_lines.iter().enumerate() {
        let y = 682.0 - i as f64 * 15.0;
        // Line number
        c.push_str(&format!("0.6 0.6 0.6 rg\n"));
        text_at(c, 76.0, y, &format!("{:>3}", i + 1));
        // Code tokens
        let x = 100.0;
        if !kw.is_empty() {
            let kx = x + indent.len() as f64 * 5.4;
            if kw.starts_with("//") {
                c.push_str("0.40 0.55 0.40 rg\n");
                text_at(c, kx, y, &format!("{}{}", kw, code));
            } else if *kw == "impl" || *kw == "fn" || *kw == "let" || *kw == "self" || *kw == "as" {
                c.push_str("0.15 0.30 0.70 rg\n");
                text_at(c, kx, y, kw);
                if !code.is_empty() {
                    let kx2 = kx + kw.len() as f64 * 5.4;
                    c.push_str("0.20 0.20 0.20 rg\n");
                    text_at(c, kx2, y, code);
                }
            } else {
                c.push_str("0.20 0.20 0.20 rg\n");
                text_at(c, kx, y, &format!("{}{}", kw, code));
            }
            if !kw2.is_empty() {
                let kx3 = x + (indent.len() + kw.len() + code.len()) as f64 * 5.4;
                if *kw2 == "as" || *kw2 == "self" {
                    c.push_str("0.15 0.30 0.70 rg\n");
                    text_at(c, kx3, y, kw2);
                }
                if !rest.is_empty() {
                    let kx4 = kx3 + kw2.len() as f64 * 5.4;
                    c.push_str("0.20 0.20 0.20 rg\n");
                    text_at(c, kx4, y, rest);
                }
            }
        } else if !code.is_empty() {
            let kx = x + indent.len() as f64 * 5.4;
            c.push_str("0.20 0.20 0.20 rg\n");
            text_at(c, kx, y, code);
        }
    }
    c.push_str("ET\n");

    // Description below code block
    c.push_str("BT /F1 10 Tf 0.25 0.25 0.25 rg\n");
    text_at(
        c,
        72.0,
        400.0,
        "The rendering splits into two passes to avoid interleaving DrawVector",
    );
    text_at(
        c,
        72.0,
        385.0,
        "begin/end calls with DrawText operations, which would cause geometry loss.",
    );
    c.push_str("ET\n");

    // Mini color palette at bottom
    c.push_str("BT /F2 12 Tf 0.3 0.3 0.3 rg\n");
    text_at(c, 72.0, 340.0, "Syntax Highlighting Palette");
    c.push_str("ET\n");
    let palette: &[(&str, f64, f64, f64)] = &[
        ("Keywords", 0.15, 0.30, 0.70),
        ("Comments", 0.40, 0.55, 0.40),
        ("Strings", 0.72, 0.20, 0.15),
        ("Types", 0.50, 0.30, 0.65),
        ("Code", 0.20, 0.20, 0.20),
    ];
    for (i, &(name, r, g, b)) in palette.iter().enumerate() {
        let x = 72.0 + i as f64 * 96.0;
        c.push_str(&format!("{:.2} {:.2} {:.2} rg\n", r, g, b));
        c.push_str(&format!("{:.0} 310 80 18 re f\n", x));
        c.push_str("BT /F1 8 Tf 1 1 1 rg\n");
        text_at(c, x + 8.0, 315.0, name);
        c.push_str("ET\n");
    }
}

/// Simple PDF writer for generating test documents.
struct PdfWriter {
    objects: Vec<String>,
    pages: Vec<usize>,         // object numbers of page objects
    page_contents: Vec<usize>, // object numbers of content stream objects
}

impl PdfWriter {
    fn new() -> Self {
        Self {
            objects: Vec::new(),
            pages: Vec::new(),
            page_contents: Vec::new(),
        }
    }

    fn add_obj(&mut self, content: String) -> usize {
        self.objects.push(content);
        self.objects.len() // 1-based object number
    }

    fn add_page(&mut self, width: f64, height: f64, content_stream: &str) {
        let content_bytes = content_stream.as_bytes();
        let content_obj = self.add_obj(format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            content_bytes.len(),
            content_stream
        ));
        self.page_contents.push(content_obj);

        // Page object references will be fixed up in finish()
        // Use placeholder - page objects are added after we know the pages catalog obj num
        self.pages.push(0); // placeholder
        let _ = (width, height); // stored implicitly
    }

    fn finish(self) -> Vec<u8> {
        // Object layout:
        // 1: Catalog
        // 2: Pages
        // 3: Font F1 (Helvetica)
        // 4: Font F2 (Helvetica-Bold)
        // 5: Font F3 (Courier)
        // 6..5+N: content streams (already added)
        // 6+N..5+2N: page objects

        let num_pages = self.pages.len();
        let num_fixed = 5; // catalog + pages + 3 fonts

        let mut all_objects: Vec<String> = Vec::new();

        // Obj 1: Catalog
        all_objects.push("<< /Type /Catalog /Pages 2 0 R >>".to_string());

        // Obj 2: Pages (will reference page objects)
        let first_page_obj = num_fixed + 1 + num_pages; // after fixed objs + content_streams
        let kids: Vec<String> = (0..num_pages)
            .map(|i| format!("{} 0 R", first_page_obj + i))
            .collect();
        all_objects.push(format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            num_pages
        ));

        // Obj 3: Font F1 - Helvetica (regular)
        all_objects.push(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .to_string(),
        );

        // Obj 4: Font F2 - Helvetica-Bold
        all_objects.push(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"
                .to_string(),
        );

        // Obj 5: Font F3 - Courier (monospace)
        all_objects.push(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>"
                .to_string(),
        );

        // Obj 6..5+num_pages: content streams
        for obj_str in &self.objects {
            all_objects.push(obj_str.clone());
        }

        // Page objects
        for i in 0..num_pages {
            let content_obj = num_fixed + 1 + i; // 1-based
            all_objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R >> >> >>",
                content_obj
            ));
        }

        // Now serialize
        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

        let mut offsets = Vec::new();
        for (i, obj_str) in all_objects.iter().enumerate() {
            offsets.push(out.len());
            let header = format!("{} 0 obj\n", i + 1);
            out.extend_from_slice(header.as_bytes());
            out.extend_from_slice(obj_str.as_bytes());
            out.extend_from_slice(b"\nendobj\n");
        }

        // Xref table
        let xref_offset = out.len();
        out.extend_from_slice(b"xref\n");
        out.extend_from_slice(format!("0 {}\n", all_objects.len() + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for offset in &offsets {
            out.extend_from_slice(format!("{:010} 00000 n \n", offset).as_bytes());
        }

        // Trailer
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
                all_objects.len() + 1,
                xref_offset
            )
            .as_bytes(),
        );

        out
    }
}
