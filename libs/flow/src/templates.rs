use crate::wire::TemplateSummary;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    pub name: &'static str,
    pub source: &'static str,
}

pub const TEMPLATES: &[Template] = &[
    Template {
        name: "annotate",
        source: include_str!("../recipes/templates/annotate.splash"),
    },
    Template {
        name: "depth",
        source: include_str!("../recipes/templates/depth.splash"),
    },
    Template {
        name: "dream",
        source: include_str!("../recipes/templates/dream.splash"),
    },
    Template {
        name: "image-enhance",
        source: include_str!("../recipes/templates/image-enhance.splash"),
    },
    Template {
        name: "image-to-mesh",
        source: include_str!("../recipes/templates/image-to-mesh.splash"),
    },
    Template {
        name: "image-to-video",
        source: include_str!("../recipes/templates/image-to-video.splash"),
    },
    Template {
        name: "image-upscale",
        source: include_str!("../recipes/templates/image-upscale.splash"),
    },
    Template {
        name: "inpaint",
        source: include_str!("../recipes/templates/inpaint.splash"),
    },
    Template {
        name: "matte",
        source: include_str!("../recipes/templates/matte.splash"),
    },
    Template {
        name: "music",
        source: include_str!("../recipes/templates/music.splash"),
    },
    Template {
        name: "ocr",
        source: include_str!("../recipes/templates/ocr.splash"),
    },
    Template {
        name: "prompt-to-image",
        source: include_str!("../recipes/templates/prompt-to-image.splash"),
    },
    Template {
        name: "prompt-to-library",
        source: include_str!("../recipes/templates/prompt-to-library.splash"),
    },
    Template {
        name: "rig-and-motion",
        source: include_str!("../recipes/templates/rig-and-motion.splash"),
    },
    Template {
        name: "sfx",
        source: include_str!("../recipes/templates/sfx.splash"),
    },
    Template {
        name: "speech",
        source: include_str!("../recipes/templates/speech.splash"),
    },
    Template {
        name: "splat",
        source: include_str!("../recipes/templates/splat.splash"),
    },
    Template {
        name: "text-to-video",
        source: include_str!("../recipes/templates/text-to-video.splash"),
    },
    Template {
        name: "world",
        source: include_str!("../recipes/templates/world.splash"),
    },
];

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
        label: graph.label,
        brief: graph.brief,
        node_count: graph.nodes.len() as u64,
        inputs,
        outputs,
    }
}
