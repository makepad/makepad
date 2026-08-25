//! Walk-mode settings and tagged front-door entry analysis.

use crate::document::Document;
use crate::model::UpAxis;

#[derive(Clone, Copy, Debug)]
pub struct WalkSettings {
    pub eye_height: f32,
    pub body_radius: f32,
    pub step_height: f32,
    pub collision: bool,
    pub pass_openings: bool,
}

impl Default for WalkSettings {
    fn default() -> Self {
        Self {
            eye_height: 1.65,
            body_radius: 0.28,
            step_height: 0.24,
            collision: true,
            pass_openings: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WalkEntry {
    pub eye: [f32; 3],
    pub target: [f32; 3],
}

pub fn front_door_entry(document: &Document, settings: WalkSettings) -> Option<WalkEntry> {
    let door = document
        .objects()
        .iter()
        .find(|object| {
            object.semantic_class().is_some_and(|kind| kind == "door")
                && object
                    .properties
                    .get("arch.entry")
                    .and_then(crate::document::Value::text)
                    .is_some_and(|entry| matches!(entry, "front" | "main"))
        })
        .or_else(|| {
            document.objects().iter().find(|object| {
                object
                    .semantic_class()
                    .is_some_and(|class| class.eq_ignore_ascii_case("door"))
            })
        })?;
    let origin = [
        door.transform.matrix.v[12],
        door.transform.matrix.v[13],
        door.transform.matrix.v[14],
    ];
    let (eye, target) = match document.up_axis() {
        UpAxis::Z => (
            [origin[0], origin[1] - 1.2, origin[2] + settings.eye_height],
            [origin[0], origin[1] + 1.0, origin[2] + settings.eye_height],
        ),
        UpAxis::Y => (
            [origin[0], origin[1] + settings.eye_height, origin[2] + 1.2],
            [origin[0], origin[1] + settings.eye_height, origin[2] - 1.0],
        ),
    };
    Some(WalkEntry { eye, target })
}

pub use crate::nav::walk::{collide_move, ground_below, integrate, WalkState};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{DocumentBuilder, Object, ObjectId};

    #[test]
    fn front_door_tag_selects_the_entry() {
        let mut builder = DocumentBuilder::new("house");
        let mut door = Object::new(ObjectId::default(), "Entry");
        door.properties.insert(
            "arch.kind".to_string(),
            crate::document::Value::Enum("door".to_string()),
        );
        door.properties.insert(
            "arch.entry".to_string(),
            crate::document::Value::Enum("front".to_string()),
        );
        builder.add_object(door);
        assert!(front_door_entry(&builder.finish(), WalkSettings::default()).is_some());
    }
}
