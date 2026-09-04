mod support;

use makepad_ai_hub::client::{ArtifactBytes, ContentProvider};
use makepad_ai_hub::error::AssetAiError;
use makepad_ai_hub::protocol::{
    ArtifactRefJson, GenerateRequestJson, HealthJson, JobStatusJson, ModelInfoJson,
    JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_RUNNING,
};
use makepad_ai_hub::registry::Domain;
use makepad_flow::engine::executors::gen::GenSeam;
use makepad_flow::engine::{RunEvent, RunState, Seams};
use makepad_flow::graph::{evaluate, is_canonical, write};
use makepad_flow::{Literal, PortType, Value};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use support::{FakeChat, FakeHttp};

#[test]
fn every_recipe_template_evaluates_and_round_trips() {
    let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("recipes/templates");
    let mut templates: Vec<_> = fs::read_dir(&template_dir)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", template_dir.display()))
        .map(|entry| entry.expect("cannot read template directory entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "splash"))
        .collect();
    templates.sort();
    assert!(!templates.is_empty(), "no recipe templates found");

    for path in templates {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("template file name is not UTF-8");
        println!("checking recipe template: {name}");
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{name}: cannot read template: {error}"));
        let graph = evaluate(&source, name)
            .unwrap_or_else(|error| panic!("{name}: evaluation failed: {error}"));

        if source.contains("let prompt = Input") {
            assert!(
                graph
                    .nodes
                    .iter()
                    .any(|node| node.id == "prompt" && node.kind == "input"),
                "{name}: expected an Input node named `prompt`"
            );
        }

        let mut reachable: HashSet<&str> = graph
            .nodes
            .iter()
            .filter(|node| node.kind == "input")
            .map(|node| node.id.as_str())
            .collect();
        loop {
            let before = reachable.len();
            for edge in &graph.edges {
                if reachable.contains(edge.from_node.as_str()) {
                    reachable.insert(edge.to_node.as_str());
                }
            }
            if reachable.len() == before {
                break;
            }
        }
        let outputs: Vec<_> = graph
            .nodes
            .iter()
            .filter(|node| matches!(node.kind.as_str(), "output" | "publish"))
            .collect();
        assert!(!outputs.is_empty(), "{name}: template has no terminal node");
        for output in outputs {
            assert!(
                reachable.contains(output.id.as_str()),
                "{name}: Output `{}` is not reachable from an Input",
                output.id
            );
        }

        let written = write(&graph);
        let rewritten = write(
            &evaluate(&written, name)
                .unwrap_or_else(|error| panic!("{name}: written form failed: {error}")),
        );
        assert_eq!(rewritten, written, "{name}: writer did not round-trip");
        assert!(is_canonical(&written), "{name}: written form is not canonical");
    }
}

#[test]
fn shipped_templates_are_warning_free_and_keep_typed_multi_input_edges() {
    let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("recipes/templates");
    for entry in fs::read_dir(&template_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "splash") {
            continue;
        }
        let name = path.file_name().unwrap().to_str().unwrap();
        let source = fs::read_to_string(&path).unwrap();
        let graph = evaluate(&source, name).unwrap();
        assert!(graph.warnings.is_empty(), "{name}: {:?}", graph.warnings);
    }

    let dream = evaluate(include_str!("../recipes/templates/dream.splash"), "dream.splash").unwrap();
    let keyframes: HashSet<_> = dream
        .edges
        .iter()
        .filter(|edge| edge.from_node == "image" && edge.to_node == "video")
        .map(|edge| (edge.from_port.as_str(), edge.to_port.as_str()))
        .collect();
    assert_eq!(keyframes, HashSet::from([("image", "image"), ("image", "last_frame")]));

    let music = evaluate(include_str!("../recipes/templates/music.splash"), "music.splash").unwrap();
    assert!(music.edges.iter().any(|edge| {
        edge.from_node == "lyrics"
            && edge.from_port == "text"
            && edge.to_node == "music"
            && edge.to_port == "lyrics"
    }));

    let inpaint = evaluate(include_str!("../recipes/templates/inpaint.splash"), "inpaint.splash")
        .unwrap();
    let image_edges: HashSet<_> = inpaint
        .edges
        .iter()
        .filter(|edge| edge.to_node == "inpaint" && matches!(edge.to_port.as_str(), "image" | "mask"))
        .map(|edge| (edge.from_node.as_str(), edge.to_port.as_str()))
        .collect();
    assert_eq!(image_edges, HashSet::from([("image", "image"), ("mask", "mask")]));
    let node = inpaint.nodes.iter().find(|node| node.id == "inpaint").unwrap();
    assert!(node
        .inputs
        .iter()
        .filter(|input| matches!(input.port.as_str(), "image" | "mask"))
        .all(|input| input.ty == makepad_flow::PortType::Image));
}

#[derive(Clone, Default)]
struct DomainFake {
    requests: Arc<Mutex<Vec<(Domain, GenerateRequestJson)>>>,
}

impl GenSeam for DomainFake {
    fn pick(&self, domain: &str) -> Result<Box<dyn ContentProvider>, String> {
        let domain = Domain::parse(domain)
            .ok_or_else(|| format!("template requested unknown domain `{domain}`"))?;
        Ok(Box::new(DomainProvider {
            domain,
            polls: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            requests: self.requests.clone(),
        }))
    }
}

struct DomainProvider {
    domain: Domain,
    polls: AtomicUsize,
    cancelled: AtomicBool,
    requests: Arc<Mutex<Vec<(Domain, GenerateRequestJson)>>>,
}

impl ContentProvider for DomainProvider {
    fn health(&self) -> Result<HealthJson, AssetAiError> {
        Err(AssetAiError::Unavailable("not used by template test".to_string()))
    }

    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> {
        Err(AssetAiError::Unavailable("not used by template test".to_string()))
    }

    fn request(
        &self,
        domain: Domain,
        request: &GenerateRequestJson,
    ) -> Result<String, AssetAiError> {
        assert_eq!(domain, self.domain);
        self.requests.lock().unwrap().push((domain, request.clone()));
        Ok(format!("{}-job", domain.as_str()))
    }

    fn poll(&self, job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        let running = self.polls.fetch_add(1, Ordering::Relaxed) == 0;
        Ok(domain_status(self.domain, job_id, running))
    }

    fn fetch_artifact(&self, artifact_id: &str) -> Result<ArtifactBytes, AssetAiError> {
        let index = artifact_id
            .rsplit('-')
            .next()
            .and_then(|index| index.parse::<usize>().ok())
            .unwrap_or(0);
        let (content_type, bytes) = domain_artifact(self.domain, index);
        Ok(ArtifactBytes {
            content_type: content_type.to_string(),
            bytes,
        })
    }

    fn cancel(&self, job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        self.cancelled.store(true, Ordering::Relaxed);
        let mut status = domain_status(self.domain, job_id, false);
        status.state = JOB_STATE_CANCELLED.to_string();
        Ok(status)
    }
}

fn domain_status(domain: Domain, job_id: &str, running: bool) -> JobStatusJson {
    let artifacts = if running {
        Vec::new()
    } else {
        (0..4)
            .map(|index| {
                let (content_type, bytes) = domain_artifact(domain, index);
                ArtifactRefJson {
                    id: format!("{}-{index}", domain.as_str()),
                    url: format!("/artifact/{}-{index}", domain.as_str()),
                    content_type: content_type.to_string(),
                    sha256: Some(makepad_ai_hub::sha256::sha256_hex(&bytes)),
                    byte_len: Some(bytes.len() as u64),
                }
            })
            .collect()
    };
    JobStatusJson {
        job_id: job_id.to_string(),
        state: if running {
            JOB_STATE_RUNNING.to_string()
        } else {
            JOB_STATE_DONE.to_string()
        },
        stage: running.then(|| "testpattern".to_string()),
        progress: running.then_some(0.5),
        artifacts,
        error: None,
        model: Some("domain-aware-fake".to_string()),
        queued_ms: None,
        started_ms: None,
        finished_ms: None,
        log: None,
        partial_text: None,
        live: None,
        serving: None,
        text: (!running).then(|| r#"{"caption":"test pattern","tags":[]}"#.to_string()),
    }
}

fn domain_artifact(domain: Domain, index: usize) -> (&'static str, Vec<u8>) {
    match domain {
        Domain::Video | Domain::Enhance => ("video/mp4", b"fake-mp4".to_vec()),
        Domain::Audio | Domain::Music | Domain::Speech => {
            ("audio/wav", b"RIFFfakeWAVE".to_vec())
        }
        Domain::Mesh
        | Domain::Paint
        | Domain::Rig
        | Domain::Motion
        | Domain::Splat
        | Domain::World => ("model/gltf-binary", b"glTFfake".to_vec()),
        Domain::Stems => ("application/zip", b"fake-stems".to_vec()),
        Domain::Notes if index > 0 => ("audio/midi", b"MThdfake".to_vec()),
        _ => {
            let pixels = vec![96_u8; 8 * 8 * 4];
            (
                "image/png",
                makepad_ai_hub::testpattern::encode_png_rgba(&pixels, 8, 8).unwrap(),
            )
        }
    }
}

fn supplied_value(ty: PortType) -> Value {
    match ty {
        PortType::Text => Value::text("test pattern prompt"),
        PortType::Json => Value::json("{}"),
        PortType::List => Value::list("[]"),
        PortType::Image => {
            let pixels = vec![160_u8; 8 * 8 * 4];
            Value::media(
                ty,
                "image/png",
                makepad_ai_hub::testpattern::encode_png_rgba(&pixels, 8, 8).unwrap(),
            )
        }
        PortType::Audio => Value::media(ty, "audio/wav", b"RIFFinputWAVE".to_vec()),
        PortType::Video => Value::media(ty, "video/mp4", b"input-mp4".to_vec()),
        PortType::Mesh => Value::media(ty, "model/gltf-binary", b"glTFinput".to_vec()),
        PortType::Bytes => Value::media(ty, "application/octet-stream", b"input".to_vec()),
    }
}

#[test]
fn every_recipe_template_runs_through_the_engine() {
    let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("recipes/templates");
    let mut templates: Vec<_> = fs::read_dir(template_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "splash"))
        .collect();
    templates.sort();

    for path in templates {
        let name = path.file_name().unwrap().to_str().unwrap();
        let source = fs::read_to_string(&path).unwrap();
        let mut graph = evaluate(&source, name).unwrap();
        assert!(!graph.label.trim().is_empty(), "{name}: missing label");
        assert!(!graph.brief.trim().is_empty(), "{name}: missing brief");

        let terminal_ids: Vec<_> = graph
            .nodes
            .iter()
            .filter(|node| matches!(node.kind.as_str(), "output" | "publish"))
            .map(|node| node.id.clone())
            .collect();
        assert!(!terminal_ids.is_empty(), "{name}: missing terminal output");

        let inputs: BTreeMap<_, _> = graph
            .nodes
            .iter()
            .filter(|node| node.kind == "input")
            .map(|node| {
                let output = node.outputs.first().expect("Input has an output");
                (
                    node.id.clone(),
                    BTreeMap::from([(output.name.clone(), supplied_value(output.ty))]),
                )
            })
            .collect();

        // Publishing itself has its own route/executor tests. Project it to an
        // in-memory terminal here so this exhaustive wiring test has no server.
        for node in &mut graph.nodes {
            if node.kind == "publish" {
                let ty = node
                    .inputs
                    .iter()
                    .find(|input| input.port == "value")
                    .expect("Publish has a value input")
                    .ty;
                node.kind = "output".to_string();
                node.type_name = "Output".to_string();
                node.params = vec![("type".to_string(), Literal::Id(ty.as_str().to_string()))];
            }
        }
        let projected_source = write(&graph);
        graph = evaluate(&projected_source, name)
            .unwrap_or_else(|error| panic!("{name}: publish projection failed: {error}"));

        let gen = DomainFake::default();
        let requests = gen.requests.clone();
        let events = support::run_graph(
            &projected_source,
            graph,
            Seams {
                chat: Arc::new(FakeChat::done(
                    r#"{"caption":"test pattern","tags":[]}"#,
                )),
                gen: Arc::new(gen),
                http: Arc::new(FakeHttp::json(200, "{}")),
            },
            None,
            inputs,
        );
        let Some(RunEvent::RunFinished { state, outputs, .. }) = events.last() else {
            panic!("{name}: no finished event: {events:#?}");
        };
        assert_eq!(*state, RunState::Done, "{name}: {events:#?}");
        assert_eq!(outputs.len(), terminal_ids.len(), "{name}: {outputs:#?}");
        for terminal in &terminal_ids {
            assert!(
                outputs.iter().any(|(node, _)| node == terminal),
                "{name}: terminal `{terminal}` produced no value"
            );
        }
        for (domain, request) in requests.lock().unwrap().iter() {
            if *domain == Domain::Inpaint {
                let names: HashSet<_> = request
                    .inputs
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|input| input.name.as_str())
                    .collect();
                assert_eq!(names, HashSet::from(["image", "mask"]), "{name}");
            }
            if *domain == Domain::Paint {
                let names: HashSet<_> = request
                    .inputs
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|input| input.name.as_str())
                    .collect();
                assert_eq!(
                    names,
                    HashSet::from(["mesh", "reference_image"]),
                    "{name}"
                );
            }
            if matches!(
                domain,
                Domain::Edit
                    | Domain::Inpaint
                    | Domain::Control
                    | Domain::Upscale
                    | Domain::Matte
                    | Domain::Depth
                    | Domain::Body
                    | Domain::Segment
                    | Domain::Enhance
                    | Domain::Rig
                    | Domain::Motion
                    | Domain::Stt
                    | Domain::Beats
                    | Domain::Stems
                    | Domain::Notes
                    | Domain::Mesh
                    | Domain::Paint
                    | Domain::Splat
                    | Domain::Vision
            ) {
                assert!(
                    request.input_b64.is_some() || request.inputs.as_ref().is_some_and(|v| !v.is_empty()),
                    "{name}: {} request lost its media input",
                    domain.as_str()
                );
            }
            if name == "image-to-video.splash" && *domain == Domain::Video {
                assert!(
                    request.input_b64.is_some(),
                    "{name}: image input was not primary"
                );
            }
        }
    }
}
