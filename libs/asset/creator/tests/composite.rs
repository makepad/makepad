use makepad_asset_creator::{character, composite::{self, Publisher, Recipe}, runner::*, tools::CreatorTools};
use makepad_asset_creator::makepad_ai_hub::{client::{ArtifactBytes, ContentProvider}, error::AssetAiError, protocol::*, registry::Domain};
use makepad_asset_chat::{session::{CancelFlag, ExecCtx, Origin, SessionId, ToolExecutor}, tools::{ContentGenerateKind as Kind, ContentToolCall}, wire::ToolOutcome};
use makepad_asset_client::json::{self, Value};
use std::{cell::RefCell, collections::HashSet, rc::Rc, time::Duration};

#[derive(Default)]
struct State {
    requests: Vec<(String, GenerateRequestJson)>,
    published: Vec<(String, Vec<u8>)>,
    cancelled: Vec<String>,
    unavailable: Option<String>,
    failure: Option<String>,
    cancel_on_request: Option<CancelFlag>,
    cancel_on_fetch: Option<CancelFlag>,
    invalid_rig: bool,
    phantom_publish: bool,
    external_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}
struct Transport(Rc<RefCell<State>>);
struct Provider { state: Rc<RefCell<State>>, domain: String }
struct Publish(Rc<RefCell<State>>);
impl GenerationTransport for Transport {
    fn route(&self, domain: &str, _: &GenerateRequestJson) -> Result<RoutedProvider, CreateError> {
        if self.0.borrow().unavailable.as_deref() == Some(domain) {
            return Err(CreateError::Unavailable(format!("no runtime for {domain}")));
        }
        Ok(RoutedProvider { provider: Box::new(Provider { state: self.0.clone(), domain: domain.into() }),
            model: format!("fake-{domain}"), node: "fake-local".into() })
    }
}
fn status(id: &str) -> JobStatusJson {
    JobStatusJson { job_id: id.into(), state: JOB_STATE_DONE.into(), stage: None, progress: None,
        artifacts: vec![], error: None, model: None, queued_ms: None, started_ms: None, finished_ms: None,
        log: None, partial_text: None, live: None, serving: None, text: None }
}
fn skin(clips: bool) -> Vec<u8> {
    skin_with(if clips { &["idle", "walk", "run", "jump"] } else { &[] }, true,
        [1.0, 0.0, 0.0, 0.0], [0; 4])
}
fn skin_with(names: &[&str], atlas: bool, weights: [f32; 4], joints: [u16; 4]) -> Vec<u8> {
    use makepad_gltf::*;
    let clips = names.iter().map(|name| GlbAnimClip {
        name: name.to_string(), channels: vec![GlbAnimChannel { joint: 0, path: GlbAnimPath::Translation,
            times: vec![0.0, 1.0], values: vec![0.0; 6] }],
    }).collect::<Vec<_>>();
    let png = makepad_asset_creator::makepad_ai_hub::makepad_base64::base64_decode(
        b"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=").unwrap();
    write_glb_mesh_skinned(&GlbSkinnedMesh {
        positions: &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: None, uvs: Some(&[[0.0; 2]; 3]), indices: &[0, 1, 2], joints_0: &[joints; 3],
        weights_0: &[weights; 3],
        joints: &[GlbJoint::at("root", None, [0.0; 3], [0.0; 3])], clips: &clips,
        base_color_png: atlas.then_some(png.as_slice()),
    })
}
fn bytes(domain: &str) -> ArtifactBytes {
    match domain {
        "rig" | "motion" => ArtifactBytes { content_type: "model/gltf-binary".into(), bytes: skin(domain == "motion") },
        "mesh" => ArtifactBytes { content_type: "model/gltf-binary".into(), bytes: b"mesh-from-image".to_vec() },
        "audio" | "speech" | "music" => ArtifactBytes { content_type: "audio/wav".into(), bytes: b"real-audio-output".to_vec() },
        "world" => ArtifactBytes { content_type: "application/x-ply".into(), bytes: b"real-splat-output".to_vec() },
        "video" => ArtifactBytes { content_type: "video/mp4".into(), bytes: b"real-video-output".to_vec() },
        _ => ArtifactBytes { content_type: "image/png".into(), bytes: domain.as_bytes().to_vec() },
    }
}
impl ContentProvider for Provider {
    fn health(&self) -> Result<HealthJson, AssetAiError> { unreachable!() }
    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> { unreachable!() }
    fn request(&self, _: Domain, request: &GenerateRequestJson) -> Result<String, AssetAiError> {
        self.state.borrow_mut().requests.push((self.domain.clone(), request.clone()));
        if let Some(cancel) = &self.state.borrow().cancel_on_request { cancel.cancel(); }
        if let Some(cancel) = &self.state.borrow().external_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(format!("job-{}", self.domain))
    }
    fn poll(&self, id: &str) -> Result<JobStatusJson, AssetAiError> {
        assert_eq!(id, format!("job-{}", self.domain), "poll owns the submitted job");
        let mut status = status(id);
        if self.state.borrow().failure.as_deref() == Some(&self.domain) {
            status.state = JOB_STATE_ERROR.into();
            status.error = Some("injected stage failure".into());
        } else if self.domain == "text" {
            let prompt = self.state.borrow().requests.last().unwrap().1.prompt.clone().unwrap();
            status.text = Some(format!("{prompt}, an original full body figure with clearly separated limbs and a readable silhouette, evenly lit against a plain background, preserving every requested appearance detail in a relaxed pose suitable for reconstruction."));
        } else {
            let output = bytes(&self.domain);
            status.artifacts.push(ArtifactRefJson { id: format!("artifact-{}", self.domain), url: "unused".into(),
                content_type: output.content_type, byte_len: None, sha256: None });
        }
        Ok(status)
    }
    fn fetch_artifact(&self, id: &str) -> Result<ArtifactBytes, AssetAiError> {
        assert_eq!(id, format!("artifact-{}", self.domain));
        if let Some(cancel) = &self.state.borrow().cancel_on_fetch { cancel.cancel(); }
        if self.domain == "rig" && self.state.borrow().invalid_rig {
            return Ok(ArtifactBytes { content_type: "model/gltf-binary".into(), bytes: rigid() });
        }
        Ok(bytes(&self.domain))
    }
    fn cancel(&self, id: &str) -> Result<JobStatusJson, AssetAiError> {
        self.state.borrow_mut().cancelled.push(id.into());
        Ok(status(id))
    }
}
impl Publisher for Publish {
    fn publish(&self, output: GeneratedBytes, _: u64, cancel: &dyn Cancellation) -> Result<Generated, CreateError> {
        check_cancel(cancel)?;
        let domain = output.kind.domain.to_string();
        self.0.borrow_mut().published.push((domain.clone(), output.artifact.as_ref().map(|b| b.bytes.clone()).unwrap_or_default()));
        let asset = output.artifact.is_some() && !self.0.borrow().phantom_publish;
        Ok(Generated { asset_id: asset.then(|| format!("asset-{domain}")),
            revision: asset.then(|| format!("rev-{domain}")), alias: asset.then(|| format!("test/{domain}")), text: output.text })
    }
}
fn run(recipe: &Recipe, state: &Rc<RefCell<State>>, cancel: &CancelFlag) -> Result<composite::CompositeResult, CreateError> {
    composite::run(recipe, 41, &Transport(state.clone()), &Publish(state.clone()), cancel, &mut |_, _| {}, Duration::ZERO)
}
fn decode(request: &GenerateRequestJson) -> Vec<u8> {
    makepad_asset_creator::makepad_ai_hub::makepad_base64::base64_decode(request.input_b64.as_ref().unwrap().as_bytes()).unwrap()
}
fn tool(state: &Rc<RefCell<State>>, call: &ContentToolCall, cancel: &CancelFlag) -> ToolOutcome {
    let mut tools = CreatorTools::with_runtime(Box::new(Transport(state.clone())), Box::new(Publish(state.clone())));
    let origin = Origin { principal: "test".into(), session: SessionId::parse("chat_0123456789abcdef").unwrap() };
    tools.execute(call, &ExecCtx { origin: &origin, known: &HashSet::new() }, &mut |_, _| {}, cancel)
}

#[test]
fn character_owns_jobs_relays_nearest_bytes_and_preserves_every_output() {
    let state = Rc::new(RefCell::new(State::default()));
    let result = run(&Recipe::content(Kind::Character, "alpine dragon", Some(2.0)), &state, &CancelFlag::default()).unwrap();
    let s = state.borrow();
    assert_eq!(s.requests.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), character::CHARACTER_DOMAINS);
    assert_eq!(decode(&s.requests[2].1), b"image");
    assert_eq!(decode(&s.requests[3].1), b"matte");
    assert_eq!(decode(&s.requests[4].1), b"mesh-from-image");
    assert_eq!(decode(&s.requests[5].1), skin(false));
    for (index, ((domain, request), (_, pin))) in s.requests.iter().zip(character::CHARACTER_PINS).enumerate() {
        assert_eq!(&request.model, pin, "{domain}");
        assert_eq!(request.seed, Some(41 + index as u64));
    }
    assert_eq!(s.requests[0].1.identity_anchor.as_deref(), Some("alpine dragon"));
    assert_eq!(s.requests[0].1.target_domain.as_deref(), Some("rig"));
    assert_eq!(s.published.len(), 6);
    assert_eq!(result.stages.last().unwrap().output.alias.as_deref(), Some("test/motion"));
    assert!(result.character.unwrap().playable);
    assert_eq!(result.dim_height, Some(2.0));
    assert_ne!(s.requests[1].1.height, Some(2), "metres never become image pixels");
}

#[test]
fn content_and_character_tools_use_the_same_shipping_executor() {
    for name in ["content.generate", "character.generate"] {
        let args = if name == "content.generate" { json::obj(vec![("kind", json::s("character")), ("prompt", json::s("dragon"))]) }
            else { json::obj(vec![("prompt", json::s("dragon"))]) };
        let call = ContentToolCall::parse(name, &args).unwrap();
        let state = Rc::new(RefCell::new(State::default()));
        let ToolOutcome::Ok { value } = tool(&state, &call, &CancelFlag::default()) else { panic!("generation failed") };
        assert!(value.get("queued").is_none());
        assert_eq!(value.get("alias").and_then(Value::as_str), Some("test/motion"));
        assert_eq!(value.get("stages").unwrap().as_arr().unwrap().len(), 6);
    }
}

#[test]
fn prop_and_sound_translate_to_real_primitives_and_inventory_is_executable() {
    for (kind, expected) in [(Kind::Prop, vec!["image", "mesh"]), (Kind::Sound, vec!["audio"])] {
        let state = Rc::new(RefCell::new(State::default()));
        let call = ContentToolCall::ContentGenerate { kind, prompt: "original content".into(), dim_height: None };
        assert!(matches!(tool(&state, &call, &CancelFlag::default()), ToolOutcome::Ok { .. }));
        assert_eq!(state.borrow().requests.iter().map(|r| r.0.clone()).collect::<Vec<_>>(), expected);
    }
    for def in makepad_asset_chat::tools::definitions().into_iter().filter(|d| d.name.ends_with(".generate")) {
        let call = ContentToolCall::parse(def.name, &json::obj(vec![("prompt", json::s("test"))])).unwrap();
        assert!(Recipe::for_call(&call).is_some(), "{} has no executor", def.name);
        let state = Rc::new(RefCell::new(State::default()));
        let result = tool(&state, &call, &CancelFlag::default());
        assert!(matches!(result, ToolOutcome::Ok { .. }), "{}: {result:?}", def.name);
        assert!(!state.borrow().requests.is_empty(), "{} acknowledged without a job", def.name);
    }
    for kind in makepad_asset_importer::gen_kinds::GEN_KINDS {
        assert!(Domain::parse(kind.domain).is_some(), "{}", kind.domain);
    }
}

#[test]
fn image_follow_ons_are_owned_or_explicitly_unavailable() {
    for then in makepad_asset_chat::tools::GenerateThen::SLUGS {
        let call = ContentToolCall::parse("image.generate", &json::obj(vec![
            ("prompt", json::s("chair")), ("then", json::s(*then))])).unwrap();
        let state = Rc::new(RefCell::new(State::default()));
        let outcome = tool(&state, &call, &CancelFlag::default());
        if *then == "character" {
            assert!(matches!(outcome, ToolOutcome::Unavailable { .. }));
            assert!(state.borrow().requests.is_empty(), "unsupported follow-on started a partial job");
        } else {
            assert!(matches!(outcome, ToolOutcome::Ok { .. }), "{then}: {outcome:?}");
            let state = state.borrow();
            if state.requests.len() == 2 {
                assert_eq!(decode(&state.requests[1].1), b"image", "{then}");
            }
        }
    }
}

#[test]
fn unavailable_failed_and_phantom_jobs_never_report_success() {
    for (unavailable, failure, phantom, invalid) in [
        (Some("rig"), None, false, false), (None, Some("mesh"), false, false),
        (None, None, true, false), (None, None, false, true),
    ] {
        let state = Rc::new(RefCell::new(State { unavailable: unavailable.map(str::to_string),
            failure: failure.map(str::to_string), phantom_publish: phantom, invalid_rig: invalid, ..Default::default() }));
        let call = ContentToolCall::CharacterGenerate { prompt: "dragon".into(), model: None };
        let outcome = tool(&state, &call, &CancelFlag::default());
        if unavailable.is_some() { assert!(matches!(outcome, ToolOutcome::Unavailable { .. })); }
        else { assert!(matches!(outcome, ToolOutcome::Failed { .. }), "{outcome:?}"); }
        assert!(!state.borrow().requests.iter().any(|r| r.0 == "motion"));
    }
}

#[test]
fn cancellation_before_submit_during_job_and_after_fetch_stops_publication() {
    for when in 0..3 {
        let cancel = CancelFlag::default();
        let state = Rc::new(RefCell::new(State::default()));
        if when == 0 { cancel.cancel(); }
        if when == 1 { state.borrow_mut().cancel_on_request = Some(cancel.clone()); }
        if when == 2 { state.borrow_mut().cancel_on_fetch = Some(cancel.clone()); }
        let result = run(&Recipe::content(Kind::Prop, "chair", None), &state, &cancel);
        assert!(matches!(result, Err(CreateError::Cancelled)));
        assert!(state.borrow().published.is_empty());
        assert_eq!(state.borrow().requests.len(), usize::from(when != 0));
        if when != 0 { assert_eq!(state.borrow().cancelled, ["job-image"]); }
    }
}

#[test]
fn ui_signal_cancels_a_job_while_the_session_worker_is_inside_execute() {
    let signal = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let state = Rc::new(RefCell::new(State { external_cancel: Some(signal.clone()), ..Default::default() }));
    let mut tools = CreatorTools::with_runtime(Box::new(Transport(state.clone())), Box::new(Publish(state.clone())))
        .with_cancel_signal(signal);
    let origin = Origin { principal: "test".into(), session: SessionId::parse("chat_0123456789abcdef").unwrap() };
    let call = ContentToolCall::ContentGenerate { kind: Kind::Prop, prompt: "chair".into(), dim_height: None };
    let result = tools.execute(&call, &ExecCtx { origin: &origin, known: &HashSet::new() }, &mut |_, _| {}, &CancelFlag::default());
    assert!(matches!(result, ToolOutcome::Failed { .. }));
    assert_eq!(state.borrow().cancelled, ["job-image"]);
    assert!(state.borrow().published.is_empty());
}

fn rigid() -> Vec<u8> {
    use makepad_gltf::*;
    write_glb_named_parts(&[GlbNamedPart { name: "arm", positions: &[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        indices: &[0, 1, 2], pivot: [0.0; 3], color: [1.0; 4], parent: None,
        animation: Some(GlbPartAnimation { kind: GlbPartAnimationKind::Swing, axis: 1, degrees: 30.0, hz: 1.0, amp: 0.0 }) }])
}

#[test]
fn rigid_articulation_is_not_reported_as_skinning() {
    let facts = character::inspect_character(&rigid()).unwrap();
    assert!(!facts.skinned && !facts.playable);
    assert!(character::inspect_character(&skin(false)).unwrap().skinned);
    assert!(!character::inspect_character(&skin(false)).unwrap().playable);
    assert!(character::inspect_character(&skin(true)).unwrap().playable);
}

#[test]
fn playable_means_runtime_gait_aliases_and_a_decodable_embedded_atlas() {
    let rig = skin_with(&["Unarmed_Idle", "Walking_A"], true, [1.0, 0.0, 0.0, 0.0], [0; 4]);
    let facts = character::inspect_character(&rig).unwrap();
    assert!(facts.skinned && facts.animated && facts.embedded_atlas && facts.playable);
    let model = makepad_render::skin::SkinnedModel::parse_glb_validated(&rig).unwrap();
    assert!(model.gait_clips().is_some());
    let mut pose = model.rest_pose();
    model.sample_clip(1, 0.5, &mut pose);
    let mut palette = Vec::new();
    model.palette(&pose, &mut palette);
    let mut packed = Vec::new();
    model.skin_to_packed(&palette, &mut packed);
    assert!(!packed.is_empty(), "the actual renderer can sample and skin the artifact");
    let no_atlas = skin_with(&["idle", "walk"], false, [1.0, 0.0, 0.0, 0.0], [0; 4]);
    let facts = character::inspect_character(&no_atlas).unwrap();
    assert!(facts.skinned && facts.animated && !facts.embedded_atlas && !facts.playable);
    let bend = skin_with(&["bend"], true, [1.0, 0.0, 0.0, 0.0], [0; 4]);
    let facts = character::inspect_character(&bend).unwrap();
    assert!(facts.animated && !facts.playable, "arbitrary motion is not locomotion");
}

#[test]
fn malformed_skin_weights_and_joint_indices_are_rejected_before_publication() {
    for weights in [[f32::NAN, 0.0, 0.0, 0.0], [f32::INFINITY, 0.0, 0.0, 0.0],
        [0.0; 4], [-0.1, 1.1, 0.0, 0.0], [0.25, 0.0, 0.0, 0.0]] {
        let rig = skin_with(&["idle", "walk"], true, weights, [0; 4]);
        assert!(character::inspect_character(&rig).unwrap_err().contains("weights"), "{weights:?}");
    }
    let rig = skin_with(&["idle", "walk"], true, [1.0, 0.0, 0.0, 0.0], [1, 0, 0, 0]);
    assert!(character::inspect_character(&rig).unwrap_err().contains("joint index"));
}
