//! JSON packet shared by single-image and live SAM 3D Body inference.

use std::fmt::Write;

#[derive(Clone, Debug, PartialEq)]
pub struct BodyPerson {
    pub mhr: [f32; 204],
    pub global_rot: [f32; 3],
    pub cam_t: [f32; 3],
    pub shape: [f32; 45],
    pub expr: [f32; 72],
    pub focal: f32,
    pub bbox: [f32; 4],
    pub kp3d: Vec<f32>,
    pub kp2d: Vec<f32>,
    pub joints: Option<Vec<f32>>,
    pub rots: Option<Vec<f32>>,
    /// Present when the hands pass ran: which hands were trusted and fused
    /// into the pose (left, right) and their boxes in full-image pixels.
    pub hands: Option<PersonHands>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PersonHands {
    pub fused: [bool; 2],
    pub boxes: [[f32; 4]; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct BodyPacket {
    pub people: Vec<BodyPerson>,
    pub ms: f32,
}

impl BodyPacket {
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        out.push_str("{\"n_people\":");
        let _ = write!(out, "{}", self.people.len());
        out.push_str(",\"people\":[");
        for (index, person) in self.people.iter().enumerate() {
            if index != 0 {
                out.push(',');
            }
            push_person(&mut out, person);
        }
        out.push_str("],\"ms\":");
        push_f32(&mut out, self.ms);
        out.push('}');
        out
    }
}

fn push_person(out: &mut String, person: &BodyPerson) {
    out.push_str("{\"mhr\":");
    push_f32s(out, &person.mhr);
    out.push_str(",\"global_rot\":");
    push_f32s(out, &person.global_rot);
    out.push_str(",\"cam_t\":");
    push_f32s(out, &person.cam_t);
    out.push_str(",\"shape\":");
    push_f32s(out, &person.shape);
    out.push_str(",\"expr\":");
    push_f32s(out, &person.expr);
    out.push_str(",\"focal\":");
    push_f32(out, person.focal);
    out.push_str(",\"bbox\":");
    push_f32s(out, &person.bbox);
    out.push_str(",\"kp3d\":");
    push_f32s(out, &person.kp3d);
    out.push_str(",\"kp2d\":");
    push_f32s(out, &person.kp2d);
    if let Some(joints) = &person.joints {
        out.push_str(",\"joints\":");
        push_f32s(out, joints);
    }
    if let Some(rots) = &person.rots {
        out.push_str(",\"rots\":");
        push_f32s(out, rots);
    }
    if let Some(hands) = &person.hands {
        out.push_str(",\"hands\":{\"fused\":[");
        out.push_str(if hands.fused[0] { "true" } else { "false" });
        out.push(',');
        out.push_str(if hands.fused[1] { "true" } else { "false" });
        out.push_str("],\"boxes\":[");
        push_f32s(out, &hands.boxes[0]);
        out.push(',');
        push_f32s(out, &hands.boxes[1]);
        out.push_str("]}");
    }
    out.push('}');
}

fn push_f32s(out: &mut String, values: &[f32]) {
    out.push('[');
    for (index, &value) in values.iter().enumerate() {
        if index != 0 {
            out.push(',');
        }
        push_f32(out, value);
    }
    out.push(']');
}

fn push_f32(out: &mut String, value: f32) {
    if !value.is_finite() {
        out.push_str("null");
        return;
    }
    let start = out.len();
    let _ = write!(out, "{value:.4}");
    while out.as_bytes().last() == Some(&b'0') {
        out.pop();
    }
    if out.as_bytes().last() == Some(&b'.') {
        out.push('0');
    }
    debug_assert!(out.len() > start);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_field_order_optional_fields_and_rounding() {
        let mut mhr = [0.0; 204];
        mhr[..4].copy_from_slice(&[1.23456, -2.34567, 3.0, -0.00001]);
        let packet = BodyPacket {
            people: vec![BodyPerson {
                mhr,
                global_rot: [0.1, 0.2, 0.3],
                cam_t: [1.0, 2.0, 3.0],
                shape: [0.0; 45],
                expr: [0.0; 72],
                focal: 1234.56789,
                bbox: [1.0, 2.0, 30.0, 40.0],
                kp3d: vec![0.12345; 70 * 3],
                kp2d: vec![5.67894; 70 * 2],
                joints: Some(vec![0.25; 127 * 3]),
                rots: None,
                hands: None,
            }],
            ms: 12.34567,
        };
        let json = packet.to_json();
        assert!(json.starts_with(
            "{\"n_people\":1,\"people\":[{\"mhr\":[1.2346,-2.3457,3.0,-0.0,"
        ));
        assert!(json.contains("\"joints\":[0.25"));
        assert!(!json.contains("\"rots\""));
        assert!(json.ends_with("],\"ms\":12.3457}"));
    }
}
