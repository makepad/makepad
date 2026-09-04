use super::*;
use makepad_ai_hub::client::ArtifactBytes;
use makepad_ai_hub::protocol::{HealthJson, ModelInfoJson, JobStatusJson};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
struct State { pending: usize, posts: usize, polls: usize, cancelled: Vec<String>, cancel_during_post: bool }
struct Provider { state: Rc<RefCell<State>>, cancel: Arc<AtomicBool> }
struct Routing { state: Rc<RefCell<State>>, cancel: Arc<AtomicBool> }
impl GenerationTransport for Routing {
    fn route(&self, _: &str, _: &GenerateRequestJson) -> Result<RoutedProvider, CreateError> {
        Ok(RoutedProvider { provider: Box::new(Provider { state: self.state.clone(), cancel: self.cancel.clone() }),
            node: "fixture-node".into(), model: "fixture-model".into() })
    }
}
impl ContentProvider for Provider {
    fn health(&self) -> Result<HealthJson, AssetAiError> { unreachable!() }
    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> { unreachable!() }
    fn request(&self, _: Domain, _: &GenerateRequestJson) -> Result<String, AssetAiError> {
        let mut state = self.state.borrow_mut(); state.posts += 1;
        if state.cancel_during_post { self.cancel.store(true, Ordering::Relaxed); }
        Ok("owned-job".into())
    }
    fn request_pending(&self, domain: Domain, wire: &GenerateRequestJson,
        cancelled: &dyn Fn() -> bool, progress: &mut dyn FnMut(&str)) -> Result<String, AssetAiError> {
        self.state.borrow_mut().pending += 1;
        progress("waiting for fixture-node admission: queue full");
        if cancelled() { return Err(AssetAiError::Cancelled); }
        self.request(domain, wire)
    }
    fn poll(&self, _: &str) -> Result<JobStatusJson, AssetAiError> {
        self.state.borrow_mut().polls += 1;
        Err(AssetAiError::Http("fixture-node: accepted poll disconnected".into()))
    }
    fn fetch_artifact(&self, _: &str) -> Result<ArtifactBytes, AssetAiError> { unreachable!() }
    fn cancel(&self, id: &str) -> Result<JobStatusJson, AssetAiError> {
        self.state.borrow_mut().cancelled.push(id.into());
        Err(AssetAiError::Cancelled)
    }
}

#[test]
fn shipping_runner_uses_pending_seam_and_cancels_accepted_jobs_without_resubmitting() {
    for when in 0..3 {
        let cancel = Arc::new(AtomicBool::new(false));
        let state = Rc::new(RefCell::new(State { cancel_during_post: when == 1, ..Default::default() }));
        let routing = Routing { state: state.clone(), cancel: cancel.clone() };
        let body = makepad_asset_client::json::obj(vec![("prompt", makepad_asset_client::json::s("test video"))]);
        let (_, request, wire) = translate("video.generate", &body, 42).unwrap();
        let mut notes = vec![];
        let result = generate_request(request, wire, &routing, &cancel, &mut |note, permille| {
            notes.push((note.to_string(), permille));
            if when == 2 { cancel.store(true, Ordering::Relaxed); }
        }, Duration::ZERO);
        let state = state.borrow();
        assert_eq!(state.pending, 1);
        assert!(notes[0].0.contains("queue full"));
        assert_eq!(notes[0].1, 0);
        if when == 2 {
            assert!(matches!(result, Err(CreateError::Cancelled)));
            assert_eq!(state.posts, 0);
            assert!(state.cancelled.is_empty());
        } else {
            assert_eq!(state.posts, 1);
            assert_eq!(state.cancelled, ["owned-job"]);
            if when == 1 {
                assert!(matches!(result, Err(CreateError::Cancelled)));
                assert_eq!(state.polls, 0);
            } else {
                assert!(matches!(result, Err(CreateError::Failed(ref error)) if error.contains("accepted poll disconnected")));
                assert_eq!(state.polls, 1);
            }
        }
    }
}
