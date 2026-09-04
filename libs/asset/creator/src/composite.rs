//! Owned composite execution over the same primitive runner, wire translator
//! and publication path as other creator surfaces. Blocking: call on a worker.
use crate::character::{self, CharacterMetadata, CHARACTER_DOMAINS, CHARACTER_PINS};
use crate::runner::{self, Cancellation, CreateError, Generated, GeneratedBytes, GenerationTransport, PublishTarget};
use makepad_asset_chat::tools::{ContentGenerateKind, ContentToolCall, GenerateThen};
use makepad_asset_client::json::{self, Value};
use makepad_asset_importer::gen_kinds::{kind_for_domain, kind_of, InputNeed};
use makepad_asset_importer::gen_publish::{GenInput, GenRequest, wire_request};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Recipe {
    pub domains: Vec<String>,
    pub prompt: String,
    /// Parameters are scoped to domains; an image's height is never metres.
    pub parameters: Vec<(String, Value)>,
    pub dim_height: Option<f64>,
}

impl Recipe {
    pub fn content(kind: ContentGenerateKind, prompt: &str, dim_height: Option<f64>) -> Self {
        let domains: &[&str] = match kind {
            ContentGenerateKind::Character => CHARACTER_DOMAINS,
            ContentGenerateKind::Prop => &["image", "mesh"],
            ContentGenerateKind::Sound => &["audio"],
        };
        let mut recipe = Self { domains: domains.iter().map(|s| s.to_string()).collect(),
            prompt: prompt.into(), parameters: Vec::new(), dim_height };
        if kind == ContentGenerateKind::Character {
            for (domain, model) in CHARACTER_PINS {
                recipe.set(domain, "model", json::s(*model));
            }
            recipe.set("mesh", "decimation_target", Value::Int(20_000));
            recipe.set("mesh", "texture", Value::Bool(true));
        }
        recipe
    }

    fn set(&mut self, domain: &str, key: &str, value: Value) {
        if !self.parameters.iter().any(|(d, _)| d == domain) {
            self.parameters.push((domain.into(), Value::Obj(Vec::new())));
        }
        if let Value::Obj(fields) = &mut self.parameters.iter_mut().find(|(d, _)| d == domain).unwrap().1 {
            fields.retain(|(k, _)| k != key);
            fields.push((key.into(), value));
        }
    }

    /// Every advertised generation verb terminates here, never in the store's
    /// acknowledgement path. Unsupported follow-ons get an explicit refusal.
    pub fn for_call(call: &ContentToolCall) -> Option<Result<Self, CreateError>> {
        use ContentToolCall as C;
        let (mut recipe, model_domain, model) = match call {
            C::ContentGenerate { kind, prompt, dim_height } =>
                return Some(Ok(Self::content(*kind, prompt, *dim_height))),
            C::CharacterGenerate { prompt, model } =>
                (Self::content(ContentGenerateKind::Character, prompt, None), "image", model),
            C::MeshGenerate { prompt, model, .. } =>
                (Self::content(ContentGenerateKind::Prop, prompt, None), "image", model),
            C::WorldGenerate { prompt, model, .. } =>
                (Self { domains: vec!["image".into(), "world".into()], prompt: prompt.clone(),
                    parameters: vec![], dim_height: None }, "image", model),
            C::ImageGenerate { prompt, model, then, .. } => {
                let domains = match then {
                    Some(GenerateThen::Character) => {
                        return Some(Err(CreateError::Unavailable("Use character.generate for the validated character pipeline; image→character without its brief is not supported by this executor.".into())));
                    }
                    Some(GenerateThen::Mesh) => vec!["image", "mesh"],
                    Some(GenerateThen::Video) => vec!["image", "video"],
                    Some(GenerateThen::World) => vec!["image", "world"],
                    Some(GenerateThen::Matte) => vec!["image", "matte"],
                    Some(GenerateThen::Depth) => vec!["image", "depth"],
                    _ => vec!["image"],
                };
                (Self { domains: domains.into_iter().map(str::to_string).collect(),
                    prompt: prompt.clone(), parameters: vec![], dim_height: None }, "image", model)
            }
            C::VideoGenerate { prompt, model, .. } | C::AudioGenerate { prompt, model }
            | C::SpeechGenerate { prompt, model, .. } | C::MusicGenerate { prompt, model, .. }
            => {
                let domain = call.name().split('.').next().unwrap();
                (Self { domains: vec![domain.into()], prompt: prompt.clone(), parameters: vec![], dim_height: None }, domain, model)
            }
            _ => return None,
        };
        if let Some(model) = model { recipe.set(model_domain, "model", json::s(model.clone())); }
        match call {
            C::ImageGenerate { width, height, steps, .. } | C::MeshGenerate { width, height, steps, .. }
            | C::WorldGenerate { width, height, steps, .. } | C::VideoGenerate { width, height, steps, .. } => {
                for (key, value) in [("width", width), ("height", height), ("steps", steps)] {
                    if let Some(n) = value { recipe.set(model_domain, key, Value::Int(*n as i64)); }
                }
                if let C::VideoGenerate { frames: Some(frames), .. } = call {
                    recipe.set("video", "frames", Value::Int(*frames as i64));
                }
            }
            C::SpeechGenerate { voice, prompt, .. } => {
                recipe.set("speech", "text", json::s(prompt.clone()));
                if let Some(voice) = voice { recipe.set("speech", "voice", json::s(voice.clone())); }
            }
            C::MusicGenerate { seconds, lyrics, steps, seed, .. } => {
                if let Some(n) = seconds { recipe.set("music", "seconds", Value::Int(*n as i64)); }
                if let Some(s) = lyrics { recipe.set("music", "lyrics", json::s(s.clone())); }
                if let Some(n) = steps { recipe.set("music", "steps", Value::Int(*n as i64)); }
                if let Some(n) = seed { recipe.set("music", "seed", Value::Int(*n as i64)); }
            }
            _ => {}
        }
        Some(Ok(recipe))
    }
}

/// Publication is also injected: tests run the shipping job executor without
/// touching a store, network, or GPU.
pub trait Publisher {
    fn publish(&self, output: GeneratedBytes, seed: u64, cancel: &dyn Cancellation) -> Result<Generated, CreateError>;
}
impl Publisher for PublishTarget {
    fn publish(&self, output: GeneratedBytes, seed: u64, cancel: &dyn Cancellation) -> Result<Generated, CreateError> {
        runner::publish_generated(output, seed, self, cancel, &mut |_, _| {})
            .map_err(CreateError::Failed)
    }
}

#[derive(Clone, Debug)]
pub struct StageResult {
    pub domain: String,
    pub job_id: String,
    pub model: String,
    pub output: Generated,
}
#[derive(Clone, Debug)]
pub struct CompositeResult {
    pub stages: Vec<StageResult>,
    pub character: Option<CharacterMetadata>,
    pub dim_height: Option<f64>,
}

pub fn run(
    recipe: &Recipe, seed: u64, transport: &dyn GenerationTransport, publisher: &dyn Publisher,
    cancel: &dyn Cancellation, progress: &mut dyn FnMut(&str, u16), poll_interval: Duration,
) -> Result<CompositeResult, CreateError> {
    runner::check_cancel(cancel)?;
    if recipe.domains.is_empty() { return Err(CreateError::Failed("empty recipe".into())); }
    if recipe.dim_height.is_some_and(|h| !h.is_finite() || !(0.01..=100.0).contains(&h)) {
        return Err(CreateError::Failed("dim_height must be finite and in 0.01..=100 metres".into()));
    }
    let is_character = recipe.domains.iter().any(|s| s == "rig");
    let mut prompt = recipe.prompt.clone();
    let mut previous = None;
    let mut result = CompositeResult { stages: vec![], character: None, dim_height: recipe.dim_height };
    let outcome = (|| -> Result<(), CreateError> {
    for (index, domain) in recipe.domains.iter().enumerate() {
        runner::check_cancel(cancel)?;
        let kind = if domain == "text" { kind_of("text.expand") } else { kind_for_domain(domain) }
            .ok_or_else(|| CreateError::Unavailable(format!("no primitive for {domain}")))?;
        let mut body = recipe.parameters.iter().find(|(d, _)| d == domain).map(|(_, v)| v.clone())
            .unwrap_or_else(|| json::obj(vec![]));
        if let Value::Obj(fields) = &mut body {
            fields.push(("prompt".into(), json::s(prompt.clone())));
        }
        let mut request = GenRequest::from_body(kind, &body).map_err(CreateError::Failed)?;
        request.original_prompt = Some(recipe.prompt.clone());
        if kind.input != InputNeed::None || (domain == "video" || domain == "world") && previous.is_some() {
            let input: &makepad_ai_hub::client::ArtifactBytes = previous.as_ref()
                .ok_or_else(|| CreateError::Failed(format!("{domain} needs prior artifact bytes")))?;
            let accepts = match kind.input {
                InputNeed::Mesh => input.content_type == "model/gltf-binary",
                InputNeed::Video => input.content_type == "video/mp4",
                _ => input.content_type.starts_with("image/"),
            };
            if !accepts { return Err(CreateError::Failed(format!("wrong input type for {domain}: {}", input.content_type))); }
            request.input = Some(GenInput { bytes: input.bytes.clone(), content_type: input.content_type.clone() });
        }
        let mut wire = wire_request(&request, request.model.clone());
        wire.seed = wire.seed.or(Some(seed.wrapping_add(index as u64)));
        if domain == "text" && is_character { character::configure_expansion(&mut wire, &recipe.prompt); }
        let mut beat = |note: &str, n: u16| progress(&format!("{domain}: {note}"),
            ((index * 1000 + n as usize) / recipe.domains.len()) as u16);
        let output = runner::generate_request(request, wire, transport, cancel, &mut beat, poll_interval)?;
        runner::check_cancel(cancel)?;
        let mut metadata = None;
        if domain == "text" {
            prompt = output.text.clone().filter(|s| !s.trim().is_empty())
                .ok_or_else(|| CreateError::Failed("expander finished without text".into()))?;
            if is_character { character::validate_brief(&recipe.prompt, &prompt).map_err(CreateError::Failed)?; }
        }
        if domain == "rig" || domain == "motion" {
            let bytes = &output.artifact.as_ref().ok_or_else(|| CreateError::Failed("missing GLB".into()))?.bytes;
            let facts = character::inspect_character(bytes).map_err(CreateError::Failed)?;
            if !facts.skinned || domain == "motion" && facts.clips.is_empty() {
                return Err(CreateError::Failed(format!("{domain} did not produce a skinned character{}", if domain == "motion" { " with named clips" } else { "" })));
            }
            metadata = Some(facts);
        }
        previous = output.artifact.clone().or(previous);
        let job_id = output.job_id.clone();
        let model = output.request.model.clone();
        let published = publisher.publish(output, seed.wrapping_add(index as u64), cancel)?;
        // A publishing implementation cannot turn an asset stage into an
        // acknowledgement; an exact revision and alias must actually exist.
        if kind.catalog().is_some() && (published.asset_id.is_none() || published.revision.is_none() || published.alias.is_none()) {
            return Err(CreateError::Failed(format!("{domain} publication returned no asset/revision/alias")));
        }
        result.stages.push(StageResult { domain: domain.clone(), job_id, model, output: published });
        if metadata.is_some() { result.character = metadata; }
    }
    runner::check_cancel(cancel)?;
    Ok(())
    })();
    if let Err(error) = outcome {
        let completed = result.stages.iter().filter_map(|stage| {
            Some(format!("{}: {} @ {}", stage.domain, stage.output.alias.as_deref()?, stage.output.revision.as_deref()?))
        }).collect::<Vec<_>>().join("; ");
        let suffix = if completed.is_empty() { String::new() } else { format!("; completed assets retained: {completed}") };
        return Err(match error {
            CreateError::Unavailable(reason) => CreateError::Unavailable(format!("{reason}{suffix}")),
            CreateError::Failed(reason) => CreateError::Failed(format!("{reason}{suffix}")),
            CreateError::Cancelled => CreateError::Cancelled,
        });
    }
    progress("published", 1000);
    Ok(result)
}
