/// SVG transform attribute parser.
/// Handles: matrix(), translate(), scale(), rotate(), skewX(), skewY() and chaining.
use crate::document::Transform2d;

pub fn parse_transform(s: &str) -> Transform2d {
    let mut result = Transform2d::identity();
    let s = s.trim();
    if s.is_empty() {
        return result;
    }

    let mut pos = 0;
    let bytes = s.as_bytes();

    while pos < bytes.len() {
        // Skip whitespace and commas
        while pos < bytes.len() && (bytes[pos].is_ascii_whitespace() || bytes[pos] == b',') {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }

        // Read function name
        let name_start = pos;
        while pos < bytes.len() && bytes[pos].is_ascii_alphabetic() {
            pos += 1;
        }
        let name = &s[name_start..pos];

        // Skip to (
        while pos < bytes.len() && bytes[pos] != b'(' {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }
        pos += 1; // skip '('

        // Read args until )
        let args_start = pos;
        while pos < bytes.len() && bytes[pos] != b')' {
            pos += 1;
        }
        let args_str = &s[args_start..pos];
        if pos < bytes.len() {
            pos += 1;
        } // skip ')'

        let args = parse_args(args_str);

        let t = match name {
            "matrix" if args.len() >= 6 => Transform2d {
                a: args[0],
                c: args[2],
                e: args[4],
                b: args[1],
                d: args[3],
                f: args[5],
            },
            "translate" if args.len() >= 1 => {
                let tx = args[0];
                let ty = if args.len() >= 2 { args[1] } else { 0.0 };
                Transform2d::translate(tx, ty)
            }
            "scale" if args.len() >= 1 => {
                let sx = args[0];
                let sy = if args.len() >= 2 { args[1] } else { sx };
                Transform2d::scale(sx, sy)
            }
            "rotate" if args.len() >= 1 => {
                let angle = args[0] * std::f32::consts::PI / 180.0;
                if args.len() >= 3 {
                    let cx = args[1];
                    let cy = args[2];
                    // rotate(a, cx, cy) = translate(cx,cy) rotate(a) translate(-cx,-cy)
                    Transform2d::translate(cx, cy)
                        .then(&Transform2d::rotate(angle))
                        .then(&Transform2d::translate(-cx, -cy))
                } else {
                    Transform2d::rotate(angle)
                }
            }
            "skewX" if args.len() >= 1 => {
                Transform2d::skew_x(args[0] * std::f32::consts::PI / 180.0)
            }
            "skewY" if args.len() >= 1 => {
                Transform2d::skew_y(args[0] * std::f32::consts::PI / 180.0)
            }
            _ => Transform2d::identity(),
        };

        result = result.then(&t);
    }

    result
}

fn parse_args(s: &str) -> Vec<f32> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse::<f32>().ok())
        .collect()
}
