use crate::wire::TemplateSummary;

pub const TEMPLATE_GROUPS: &[&str] = &[
    "Image",
    "Video",
    "Audio",
    "3D",
    "Vision & text",
    "Utilities",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    pub name: &'static str,
    pub group: &'static str,
    pub source: &'static str,
}

macro_rules! template {
    ($name:literal, $group:literal) => {
        Template {
            name: $name,
            group: $group,
            source: include_str!(concat!("../recipes/templates/", $name, ".splash")),
        }
    };
}

/// Bundled creator pipelines, ordered exactly as the New menu presents them.
pub const TEMPLATES: &[Template] = &[
    template!("depth", "Image"),
    template!("expanded-prompt-to-image", "Image"),
    template!("image-control", "Image"),
    template!("image-control-canny", "Image"),
    template!("image-edit", "Image"),
    template!("image-enhance", "Image"),
    template!("image-upscale", "Image"),
    template!("inpaint", "Image"),
    template!("matte", "Image"),
    template!("prompt-to-cutout", "Image"),
    template!("prompt-to-depth", "Image"),
    template!("prompt-to-image", "Image"),
    template!("prompt-to-segment", "Image"),
    template!("segment", "Image"),
    template!("sprite-enhance", "Image"),
    template!("text-to-image", "Image"),
    template!("dream", "Video"),
    template!("expanded-prompt-to-video", "Video"),
    template!("expanded-prompt-to-video-loop", "Video"),
    template!("image-to-video", "Video"),
    template!("prompt-to-video", "Video"),
    template!("prompt-to-video-keyframe", "Video"),
    template!("prompt-to-video-loop", "Video"),
    template!("text-to-video", "Video"),
    template!("video-enhance", "Video"),
    template!("video-tween", "Video"),
    template!("audio-beats", "Audio"),
    template!("audio-notes", "Audio"),
    template!("audio-stems", "Audio"),
    template!("expanded-prompt-to-music", "Audio"),
    template!("expanded-prompt-to-sfx", "Audio"),
    template!("music", "Audio"),
    template!("sfx", "Audio"),
    template!("speech", "Audio"),
    template!("speech-to-text", "Audio"),
    template!("expanded-prompt-to-mesh", "3D"),
    template!("expanded-prompt-to-world", "3D"),
    template!("image-to-mesh", "3D"),
    template!("image-to-mesh-basic", "3D"),
    template!("playable-character", "3D"),
    template!("playable-character-pbr", "3D"),
    template!("prompt-to-character", "3D"),
    template!("prompt-to-cutout-pbr-mesh", "3D"),
    template!("prompt-to-mesh", "3D"),
    template!("prompt-to-pbr-mesh", "3D"),
    template!("prompt-to-splat", "3D"),
    template!("prompt-to-world", "3D"),
    template!("rig-and-motion", "3D"),
    template!("splat", "3D"),
    template!("world", "3D"),
    template!("annotate", "Vision & text"),
    template!("body-pose", "Vision & text"),
    template!("ocr", "Vision & text"),
    template!("prompt-expand", "Vision & text"),
    template!("prompt-to-library", "Utilities"),
];

pub fn group_rank(group: &str) -> usize {
    TEMPLATE_GROUPS
        .iter()
        .position(|candidate| *candidate == group)
        .unwrap_or(TEMPLATE_GROUPS.len())
}

pub fn template(name: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|template| template.name == name)
}

pub fn template_summary(template: &Template) -> TemplateSummary {
    let file_name = format!("{}.splash", template.name);
    let graph = crate::graph::evaluate(template.source, &file_name)
        .unwrap_or_else(|error| panic!("bundled template `{}` is invalid: {error}", template.name));
    let inputs = graph
        .nodes
        .iter()
        .filter(|node| node.kind == "input")
        .filter_map(|node| {
            node.outputs
                .first()
                .map(|port| (node.id.clone(), port.ty.as_str().to_string()))
        })
        .collect();
    let outputs = graph
        .nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "output" | "publish"))
        .filter_map(|node| {
            if node.kind == "publish" {
                Some((node.id.clone(), "json".to_string()))
            } else {
                node.inputs
                    .first()
                    .map(|port| (node.id.clone(), port.ty.as_str().to_string()))
            }
        })
        .collect();
    TemplateSummary {
        name: template.name.to_string(),
        group: template.group.to_string(),
        label: graph.label,
        brief: graph.brief,
        node_count: graph.nodes.len() as u64,
        inputs,
        outputs,
    }
}
