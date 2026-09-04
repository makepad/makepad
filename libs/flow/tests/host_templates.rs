#![cfg(not(target_arch = "wasm32"))]

use makepad_flow::client::{ClientError, FlowClient};
use makepad_flow::host::{FlowServer, FlowServerConfig};
use makepad_flow::templates::TEMPLATES;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "makepad-flow-host-templates-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn template_registry_matches_the_recipe_directory() {
    let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("recipes/templates");
    let mut files: Vec<_> = std::fs::read_dir(template_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "splash"))
        .map(|path| path.file_stem().unwrap().to_str().unwrap().to_string())
        .collect();
    files.sort();
    let mut registered: Vec<_> = TEMPLATES
        .iter()
        .map(|template| template.name.to_string())
        .collect();
    registered.sort();
    assert_eq!(registered, files);
}

#[test]
fn host_lists_and_creates_templates() {
    let root = TempRoot::new();
    let mut config = FlowServerConfig::new(root.0.clone());
    config.log = Box::new(|_| {});
    let server = FlowServer::start(config).unwrap();
    let endpoints = server.endpoints();
    let client_endpoints = makepad_flow::client::Endpoints {
        control: endpoints.control,
        data: endpoints.data,
    };
    let client = FlowClient::connect(client_endpoints, endpoints.token.clone(), None).unwrap();

    let templates = client.templates().unwrap();
    assert_eq!(templates.len(), 19);
    assert!(templates.windows(2).all(|pair| pair[0].name < pair[1].name));
    assert!(templates.iter().all(|template| !template.brief.is_empty()));
    let dream = templates.iter().find(|template| template.name == "dream").unwrap();
    assert_eq!(dream.inputs, [("prompt".to_string(), "text".to_string())]);
    assert_eq!(dream.outputs, [("movie".to_string(), "video".to_string())]);
    let publish = templates
        .iter()
        .find(|template| template.name == "prompt-to-library")
        .unwrap();
    assert_eq!(publish.inputs, [("prompt".to_string(), "text".to_string())]);
    assert_eq!(
        publish.outputs,
        [("published".to_string(), "json".to_string())]
    );

    let detail = client.template("dream").unwrap();
    assert_eq!(detail.name, "dream");
    assert!(!detail.label.is_empty());
    assert!(!detail.brief.is_empty());
    assert!(detail.source.contains("Flow{"));

    let created = client.create_from_template("my-dream", "dream").unwrap();
    assert_eq!(created.revision, 1);
    assert_eq!(created.graph.label, "DREAM");
    let flows = client.flows().unwrap();
    let flow = flows.iter().find(|flow| flow.name == "my-dream").unwrap();
    assert_eq!(flow.state, "ok");

    assert!(matches!(
        client.create_from_template("my-dream", "dream"),
        Err(ClientError::Http { status: 409, .. })
    ));
    assert!(matches!(
        client.create_from_template("unknown-template", "not-shipped"),
        Err(ClientError::Http { status: 404, .. })
    ));

    server.shutdown();
}
