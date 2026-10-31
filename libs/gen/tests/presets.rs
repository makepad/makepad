//! Presets on the headless host against a scripted fleet and a results folder:
//! the character recipe sends exactly the validated composite's jobs and
//! gates on the same checks, a fixed-seed run is reused from the library
//! without a job, and the sound and music defaults are the game pins.
#![cfg(not(target_arch = "wasm32"))]

use makepad_ai_hub::client::{ArtifactBytes, ContentProvider};
use makepad_ai_hub::error::AssetAiError;
use makepad_ai_hub::job::{GenPick, GenRouter};
use makepad_ai_hub::protocol::*;
use makepad_ai_hub::registry::Domain;
use makepad_flow::engine::results::FolderStore;
use makepad_gen::host::GenHostConfig;
use makepad_gen::service::GenService;
use makepad_gen::tools::{Phase, RunRequest};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const PROMPT: &str = "alpine dragon";

#[derive(Default)]
struct State {
    requests: Vec<(Domain, GenerateRequestJson)>,
    short_brief: bool,
    rigid_rig: bool,
}

#[derive(Clone, Default)]
struct Fleet(Arc<Mutex<State>>);

impl GenRouter for Fleet {
    fn route(&self, domain: &str, request: &GenerateRequestJson, excluded: &[String]) -> Result<GenPick, String> {
        let domain = Domain::parse(domain).ok_or("unknown domain")?;
        let model = if request.model.is_empty() { format!("fake-{}", domain.as_str()) } else { request.model.clone() };
        let provider = Box::new(Provider { fleet: self.clone(), domain, jobs: Mutex::new(Vec::new()) });
        if !excluded.is_empty() {
            return Err("no alternate node".into());
        }
        Ok(GenPick { provider, base_url: "http://fake-node".into(), model, model_state: None })
    }
}

struct Provider {
    fleet: Fleet,
    domain: Domain,
    jobs: Mutex<Vec<GenerateRequestJson>>,
}

fn png(fill: u8) -> Vec<u8> {
    let mut pixels = vec![fill; 64 * 64 * 4];
    for alpha in pixels.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    makepad_ai_hub::testpattern::encode_png_rgba(&pixels, 64, 64).unwrap()
}

fn skin(clips: bool) -> Vec<u8> {
    use makepad_gltf::*;
    let names: &[&str] = if clips { &["idle", "walk", "run", "jump"] } else { &[] };
    let clips = names.iter().map(|name| GlbAnimClip {
        name: name.to_string(),
        channels: vec![GlbAnimChannel { joint: 0, path: GlbAnimPath::Translation, times: vec![0.0, 1.0], values: vec![0.0; 6] }],
    }).collect::<Vec<_>>();
    let atlas = png(200);
    write_glb_mesh_skinned(&GlbSkinnedMesh {
        positions: &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: None, uvs: Some(&[[0.0; 2]; 3]), indices: &[0, 1, 2], joints_0: &[[0; 4]; 3],
        weights_0: &[[1.0, 0.0, 0.0, 0.0]; 3],
        joints: &[GlbJoint::at("root", None, [0.0; 3], [0.0; 3])], clips: &clips,
        base_color_png: Some(atlas.as_slice()),
    })
}

fn rigid() -> Vec<u8> {
    use makepad_gltf::*;
    write_glb_named_parts(&[GlbNamedPart { name: "arm", positions: &[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        indices: &[0, 1, 2], pivot: [0.0; 3], color: [1.0; 4], parent: None,
        animation: Some(GlbPartAnimation { kind: GlbPartAnimationKind::Swing, axis: 1, degrees: 30.0, hz: 1.0, amp: 0.0 }) }])
}

impl Provider {
    fn answer(&self) -> (Option<String>, Vec<ArtifactBytes>) {
        let (text, first) = self.answer_one();
        let mut artifacts: Vec<ArtifactBytes> = first.into_iter().collect();
        // A score-planning music model answers with the song and its score.
        let scored = self.domain == Domain::Music
            && self.jobs.lock().unwrap().last().is_some_and(|request| request.model == "yue2-3b");
        if scored {
            artifacts.push(ArtifactBytes { content_type: "text/vnd.abc".into(), bytes: b"X:1\nK:C\nC4|".to_vec() });
        }
        (text, artifacts)
    }

    fn answer_one(&self) -> (Option<String>, Option<ArtifactBytes>) {
        let state = self.fleet.0.lock().unwrap();
        let bytes = |content_type: &str, bytes: Vec<u8>| Some(ArtifactBytes { content_type: content_type.into(), bytes });
        match self.domain {
            Domain::Text if state.short_brief => (Some(format!("{PROMPT}, a dragon.")), None),
            Domain::Text => (Some(format!("{PROMPT}, an original full body figure with clearly separated limbs and a readable silhouette, evenly lit against a plain background, preserving every requested appearance detail in a relaxed pose suitable for reconstruction.")), None),
            Domain::Image => (None, bytes("image/png", png(40))),
            Domain::Matte => (None, bytes("image/png", png(80))),
            Domain::Mesh => (None, bytes("model/gltf-binary", rigid())),
            Domain::Rig if state.rigid_rig => (None, bytes("model/gltf-binary", rigid())),
            Domain::Rig => (None, bytes("model/gltf-binary", skin(false))),
            Domain::Motion => (None, bytes("model/gltf-binary", skin(true))),
            Domain::Audio | Domain::Music | Domain::Speech => (None, bytes("audio/wav", wav())),
            _ => (None, bytes("image/png", png(120))),
        }
    }
}

fn wav() -> Vec<u8> {
    let frames = 2400usize;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + frames as u32 * 2).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    for value in [16u32.to_le_bytes().to_vec(), 1u16.to_le_bytes().to_vec(), 1u16.to_le_bytes().to_vec(),
        24_000u32.to_le_bytes().to_vec(), 48_000u32.to_le_bytes().to_vec(), 2u16.to_le_bytes().to_vec(), 16u16.to_le_bytes().to_vec()] {
        wav.extend_from_slice(&value);
    }
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(frames as u32 * 2).to_le_bytes());
    for n in 0..frames {
        wav.extend_from_slice(&(((n as f32 * 0.05).sin() * 8000.0) as i16).to_le_bytes());
    }
    wav
}

impl ContentProvider for Provider {
    fn health(&self) -> Result<HealthJson, AssetAiError> { Err(AssetAiError::Unavailable("fake".into())) }
    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> { Ok(Vec::new()) }
    fn request(&self, domain: Domain, request: &GenerateRequestJson) -> Result<String, AssetAiError> {
        assert_eq!(domain, self.domain);
        self.fleet.0.lock().unwrap().requests.push((domain, request.clone()));
        let mut jobs = self.jobs.lock().unwrap();
        jobs.push(request.clone());
        Ok(format!("job-{}-{}", domain.as_str(), jobs.len()))
    }
    fn poll(&self, id: &str) -> Result<JobStatusJson, AssetAiError> {
        let (text, artifact) = self.answer();
        Ok(JobStatusJson {
            job_id: id.into(), state: JOB_STATE_DONE.into(), stage: None, progress: Some(1.0),
            artifacts: artifact.iter().enumerate().map(|(index, artifact)| ArtifactRefJson {
                id: format!("{id}-artifact-{index}"), url: "unused".into(), content_type: artifact.content_type.clone(), sha256: None, byte_len: None,
            }).collect(),
            error: None, model: None, queued_ms: None, started_ms: None, finished_ms: None,
            log: None, partial_text: None, partial_artifacts: None, live: None, serving: None, text,
        })
    }
    fn fetch_artifact(&self, id: &str) -> Result<ArtifactBytes, AssetAiError> {
        let index: usize = id.rsplit('-').next().and_then(|n| n.parse().ok()).unwrap_or(0);
        self.answer().1.into_iter().nth(index).ok_or(AssetAiError::Backend("no artifact".into()))
    }
}

struct Scratch(Vec<PathBuf>);

impl Scratch {
    fn dir(&mut self, label: &str) -> PathBuf {
        // The clock alone can repeat across parallel tests (microsecond
        // resolution), so a counter keeps every directory distinct.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let count = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("makepad-gen-{}-{label}-{nonce}-{count}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        self.0.push(path.clone());
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

struct Rig {
    results: PathBuf,
    service: GenService,
    fleet: Fleet,
    _scratch: Scratch,
}

fn rig() -> Rig {
    let mut scratch = Scratch(Vec::new());
    let results = scratch.dir("results");
    let store = FolderStore::open(results.clone(), "sandbox").unwrap();
    let fleet = Fleet::default();
    let service = GenService::start(GenHostConfig {
        root: scratch.dir("host"),
        results: Arc::new(store),
        namespace: "sandbox".into(),
        router: Arc::new(fleet.clone()),
    }).unwrap();
    Rig { results, service, fleet, _scratch: scratch }
}

fn run(rig: &Rig, preset: &str, prompt: Option<&str>, seed: Option<u64>, params: Vec<(String, makepad_strict_json::Value)>) -> Result<makepad_gen::tools::JobView, String> {
    let request = RunRequest { preset: preset.into(), prompt: prompt.map(Into::into), inputs: Vec::new(), params, seed };
    rig.service.run(&request, true, &|| false, &mut |_, _| {})
}

fn decode(request: &GenerateRequestJson) -> Vec<u8> {
    makepad_ai_hub::makepad_base64::base64_decode(request.input_b64.as_ref().expect("input bytes").as_bytes()).unwrap()
}

#[test]
fn every_preset_evaluates_and_describes_its_inputs() {
    let presets = makepad_gen::catalog::presets();
    assert_eq!(presets.len(), makepad_gen::catalog::PRESETS.len());
    for info in &presets {
        assert!(!info.label.is_empty() && !info.brief.is_empty(), "{}", info.id);
        assert!(!info.generators.is_empty(), "{} runs no generator", info.id);
        assert!(info.prompt.is_some() || !info.media.is_empty(), "{} takes nothing", info.id);
    }
    let find = |id: &str| presets.iter().find(|info| info.id == id).unwrap();
    assert_eq!(find("image.edit").media.len(), 1);
    assert_eq!(find("mesh.paint").media.len(), 2);
    assert_eq!(find("song").texts.len(), 1);
    // The game pins are the sound and music defaults.
    assert_eq!(find("sound").generators[0].model, "sa3-sfx");
    assert_eq!(find("music").generators[0].model, "minimax-music3-q4");
    assert_eq!(find("character").generators.iter().map(|g| g.model.as_str()).collect::<Vec<_>>(),
        ["", "flux1-dev", "birefnet-hr", "trellis-2", "skintokens", "hy-motion"]);
}

#[test]
fn character_sends_the_validated_composites_jobs_and_is_reused_on_its_seed() {
    let rig = rig();
    let job = run(&rig, "character", Some(PROMPT), Some(41), Vec::new()).unwrap();
    assert_eq!(job.phase, Phase::Done, "{:?}", job.error);
    let requests = rig.fleet.0.lock().unwrap().requests.clone();
    assert_eq!(requests.iter().map(|(domain, _)| domain.as_str()).collect::<Vec<_>>(), ["text", "image", "matte", "mesh", "rig", "motion"]);
    let brief = format!("{PROMPT}, an original full body figure with clearly separated limbs and a readable silhouette, evenly lit against a plain background, preserving every requested appearance detail in a relaxed pose suitable for reconstruction.");
    let text = &requests[0].1;
    assert_eq!(text.model, "fake-text", "the routed text model, never a retired pin");
    assert_eq!(text.prompt.as_deref(), Some(PROMPT));
    assert_eq!(text.target_domain.as_deref(), Some("rig"));
    assert_eq!(text.identity_anchor.as_deref(), Some(PROMPT));
    assert_eq!(text.temperature, Some(0.0));
    assert_eq!(text.max_tokens, Some(512));
    assert!(text.style.as_deref().unwrap().starts_with("When the intent names an established character"));
    for ((domain, request), (index, model)) in requests.iter().zip(["fake-text", "flux1-dev", "birefnet-hr", "trellis-2", "skintokens", "hy-motion"].into_iter().enumerate()) {
        assert_eq!(request.model, model, "{}", domain.as_str());
        assert_eq!(request.seed, Some(41 + index as u64), "{}", domain.as_str());
        if index > 0 {
            assert_eq!(request.prompt.as_deref(), Some(brief.as_str()), "{}", domain.as_str());
        }
        // Full quality as the composite ran it: the fleet's own sizes, no
        // cheaper remesh, atlas or motion mode.
        assert_eq!((request.width, request.height, request.steps), (None, None, None), "{}", domain.as_str());
        assert_eq!((request.remesh_resolution, request.texture_size, request.motion_mode.as_deref()), (None, None, None), "{}", domain.as_str());
    }
    assert_eq!((requests[3].1.decimation_target, requests[3].1.texture), (Some(20_000), Some(true)));
    assert_eq!(decode(&requests[2].1), png(40), "the matte cuts out the picture");
    assert_eq!(decode(&requests[3].1), png(80), "the mesh is made from the cutout");
    assert_eq!(decode(&requests[4].1), rigid(), "the rig takes the mesh");
    assert_eq!(decode(&requests[5].1), skin(false), "motion takes the checked rig");
    let facts = job.character.as_ref().expect("character facts");
    assert_eq!(facts.get("skinned"), Some(&makepad_strict_json::Value::Bool(true)));
    assert_eq!(facts.get("clips").and_then(|clips| clips.as_arr()).map(|clips| clips.len()), Some(4));
    assert_eq!(job.assets.iter().map(|asset| asset.node.as_str()).collect::<Vec<_>>(),
        ["published", "published_image", "published_matte", "published_mesh", "published_rig"]);
    assert_eq!(job.assets[0].kind, "character");
    assert!(job.assets[0].alias.as_deref().is_some_and(|alias| alias.starts_with("sandbox/gen-")));

    // The same run again: the stored character, and no job.
    let again = run(&rig, "character", Some(PROMPT), Some(41), Vec::new()).unwrap();
    assert!(again.reused);
    assert_eq!(again.assets[0].asset_id, job.assets[0].asset_id);
    assert_eq!(again.assets[0].revision, job.assets[0].revision);
    assert_eq!(rig.fleet.0.lock().unwrap().requests.len(), 6);
    // A drawn seed never hits.
    let drawn = run(&rig, "character", Some(PROMPT), None, Vec::new()).unwrap();
    assert!(!drawn.reused);
    assert_eq!(rig.fleet.0.lock().unwrap().requests.len(), 12);
}

#[test]
fn a_bad_brief_or_an_unskinned_rig_stops_the_character_before_its_next_job() {
    let rig = rig();
    rig.fleet.0.lock().unwrap().short_brief = true;
    let job = run(&rig, "character", Some(PROMPT), Some(3), Vec::new()).unwrap();
    assert_eq!(job.phase, Phase::Failed);
    assert!(job.error.as_deref().unwrap().contains("brief is too short"), "{:?}", job.error);
    assert_eq!(rig.fleet.0.lock().unwrap().requests.len(), 1, "nothing is painted from a bad brief");
    assert!(job.assets.is_empty());

    let rig = self::rig();
    rig.fleet.0.lock().unwrap().rigid_rig = true;
    let job = run(&rig, "character", Some(PROMPT), Some(3), Vec::new()).unwrap();
    assert_eq!(job.phase, Phase::Failed);
    assert!(job.error.as_deref().unwrap().contains("did not produce a skinned character"), "{:?}", job.error);
    let domains: Vec<Domain> = rig.fleet.0.lock().unwrap().requests.iter().map(|(domain, _)| *domain).collect();
    assert!(!domains.contains(&Domain::Motion), "no motion for a rigid rig");
    assert!(job.assets.iter().all(|asset| asset.node != "published" && asset.node != "published_rig"));
}

#[test]
fn sound_music_and_song_run_with_the_game_pins() {
    use makepad_strict_json::Value as Json;
    let rig = rig();
    let sound = run(&rig, "sound", Some("a door slams"), None, vec![("seconds".into(), Json::Int(3))]).unwrap();
    assert_eq!(sound.phase, Phase::Done, "{:?}", sound.error);
    assert_eq!(sound.assets[0].kind, "audio");
    let music = run(&rig, "music", Some("tense synthwave"), None, Vec::new()).unwrap();
    assert_eq!(music.phase, Phase::Done, "{:?}", music.error);
    assert!(run(&rig, "song", Some("upbeat pop"), None, Vec::new()).unwrap_err().contains("lyrics"));
    let song = run(&rig, "song", Some("upbeat pop"), None, vec![("lyrics".into(), Json::Str("[chorus]\nGame over".into()))]).unwrap();
    assert_eq!(song.phase, Phase::Done, "{:?}", song.error);
    let requests = rig.fleet.0.lock().unwrap().requests.clone();
    assert_eq!((requests[0].1.model.as_str(), requests[0].1.seconds), ("sa3-sfx", Some(3.0)));
    assert_eq!((requests[1].1.model.as_str(), requests[1].1.lyrics.as_deref()), ("minimax-music3-q4", Some("[Instrumental]")));
    assert_eq!((requests[2].1.model.as_str(), requests[2].1.lyrics.as_deref()), ("minimax-music3-q4", Some("[chorus]\nGame over")));
    assert!(run(&rig, "sound", Some("a door"), None, vec![("volume".into(), Json::Int(3))]).unwrap_err().contains("no param `volume`"));
    assert!(run(&rig, "nope", Some("x"), None, Vec::new()).unwrap_err().contains("unknown preset"));
}

#[test]
fn song_score_sings_with_yue2_and_publishes_the_score() {
    use makepad_strict_json::Value as Json;
    let rig = rig();
    let lyrics = ("lyrics".to_string(), Json::Str("[Verse]\nNeon rain".into()));
    let job = run(&rig, "song.score", Some("city pop, female vocal"), None, vec![lyrics.clone()]).unwrap();
    assert_eq!(job.phase, Phase::Done, "{:?}", job.error);
    assert_eq!(job.assets.len(), 2, "the song and its score");
    // Re-sing an edited score: it rides on the request verbatim.
    let edited = "X:1\nK:F\nF4|";
    let job = run(&rig, "song.score", Some("jazz, female vocal"), None, vec![lyrics, ("score".into(), Json::Str(edited.into()))]).unwrap();
    assert_eq!(job.phase, Phase::Done, "{:?}", job.error);
    let requests = rig.fleet.0.lock().unwrap().requests.clone();
    let (first, second) = (&requests[0].1, &requests[1].1);
    assert_eq!(first.model, "yue2-3b");
    assert_eq!(first.score_mode.as_deref(), Some("full"));
    assert!(first.score.is_none() && first.seconds.is_none(), "no score planned in, no length cut");
    assert_eq!(second.score.as_deref(), Some(edited));
    assert_eq!(second.lyrics.as_deref(), Some("[Verse]\nNeon rain"));
}

#[test]
fn rigid_articulation_is_not_reported_as_skinning() {
    use makepad_gen::character::inspect_character;
    let facts = inspect_character(&rigid()).unwrap();
    assert!(!facts.skinned && !facts.playable);
    assert!(inspect_character(&skin(false)).unwrap().skinned);
    assert!(!inspect_character(&skin(false)).unwrap().playable);
    assert!(inspect_character(&skin(true)).unwrap().playable);
}

#[test]
fn tool_definitions_are_json_schemas_and_calls_parse_strictly() {
    use makepad_gen::tools::{parse, GenCall, TOOLS};
    use makepad_strict_json::Value as Json;
    for tool in TOOLS {
        let schema = makepad_strict_json::parse(tool.input.as_bytes()).unwrap_or_else(|error| panic!("{}: {error}", tool.name));
        assert_eq!(schema.get("type").and_then(Json::as_str), Some("object"));
        assert_eq!(tool.api_name, tool.name.replace('.', "_"));
    }
    let args = makepad_strict_json::parse(br#"{"preset": "song", "prompt": "pop", "params": {"lyrics": "la"}, "seed": 7}"#).unwrap();
    let GenCall::Run { request, wait } = parse("gen_run", &args).unwrap() else { panic!() };
    assert!(wait);
    assert_eq!((request.preset.as_str(), request.seed), ("song", Some(7)));
    assert!(parse("gen.run", &makepad_strict_json::parse(br#"{"preset": "x", "colour": 1}"#).unwrap()).unwrap_err().contains("unknown argument"));
    assert!(parse("gen.job", &makepad_strict_json::parse(br#"{}"#).unwrap()).unwrap_err().contains("`id` is required"));
    assert!(parse("gen.cancel", &makepad_strict_json::parse(br#"{"id": 3}"#).unwrap()).is_err());
}

#[test]
fn an_edit_of_a_library_picture_records_it_as_its_parent() {
    let rig = rig();
    let picture = run(&rig, "image", Some("a lighthouse"), Some(5), Vec::new()).unwrap();
    assert_eq!(picture.phase, Phase::Done, "{:?}", picture.error);
    let source = &picture.assets[0];
    let request = RunRequest { preset: "image.edit".into(), prompt: Some("make it night".into()), inputs: vec![source.asset_id.clone()], params: Vec::new(), seed: Some(9) };
    let edit = rig.service.run(&request, true, &|| false, &mut |_, _| {}).unwrap();
    assert_eq!(edit.phase, Phase::Done, "{:?}", edit.error);
    let sent = rig.fleet.0.lock().unwrap().requests.last().unwrap().1.clone();
    assert_eq!(decode(&sent), png(40), "the edit sends the library picture as its primary input");
    // The result's record: made from the source picture, under the recipe
    // digest.
    let sidecar = std::fs::read(rig.results.join(format!("{}.json", edit.assets[0].asset_id))).unwrap();
    let record = makepad_strict_json::parse(&sidecar).unwrap();
    let parents: Vec<&str> = record.get("parents").and_then(|parents| parents.as_arr()).unwrap().iter().filter_map(|parent| parent.as_str()).collect();
    assert_eq!(parents, vec![source.asset_id.as_str()]);
    assert!(record.get("digest").and_then(|digest| digest.as_str()).is_some());
    // Wrong or missing inputs never start a job.
    let count = rig.fleet.0.lock().unwrap().requests.len();
    let missing = RunRequest { inputs: Vec::new(), ..request.clone() };
    assert!(rig.service.run(&missing, true, &|| false, &mut |_, _| {}).unwrap_err().contains("takes 1 input asset"));
    assert_eq!(rig.fleet.0.lock().unwrap().requests.len(), count);
}
