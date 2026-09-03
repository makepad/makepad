pub mod engine;
pub mod graph;
pub mod instance;
pub mod values;
pub mod wire;

#[cfg(all(feature = "client", not(target_arch = "wasm32")))]
pub mod client;
#[cfg(all(feature = "client", not(target_arch = "wasm32")))]
pub mod embed;

#[cfg(all(feature = "host", not(target_arch = "wasm32")))]
pub mod host;

pub use engine::*;
pub use instance::*;
pub use values::*;
pub use wire::*;

pub const PRELUDE: &str = include_str!("prelude.splash");

/// Compact authoring doctrine returned with the prelude catalog.
pub const AUTHORING_BRIEF: &str = r#"A flow file is ordinary splash and starts with `use mod.flow.*`.
Declare every node with one `let name = Type{...}` binding.
Finish the file with one `Flow{...}` expression.
List every node in that Flow object; its field name is the stable node id.
Node ids use splash identifiers and must be unique.
An input port is a field on the node.
Its value is either a literal or another node's output reference.
An output port is selected with `node.port()`.
Use `node.out(@port)` when a generic Gen input shadows a named method.
Input nodes expose values supplied by an instance or tool caller.
Output nodes name the results returned by the flow.
Set an Input or Output `type:` to a supported port type.
The supported types are text, image, audio, video, mesh, json, list and bytes.
Edges are type-checked when the definition is evaluated.
Cycles are rejected.
Use `at: vec2(x, y)` only for canvas placement.
Use `ui:` for a node face or for the whole Flow face.
Faces are carried as source and are not executed by the graph evaluator.
Build-time splash logic may construct nodes before the final Flow expression.
Such a source is custom rather than canonical.
Canvas writes serialize the graph into the canonical flat form.
Fn declares named inputs in `in: {...}`.
Fn declares output names in `out: [@name, ...]`.
Fn `run:` is a pure splash closure evaluated at run time.
Fn closures cannot perform I/O.
Use Http for bounded network reads and writes.
Http url, headers and body are ordinary input ports.
Use Ask to park a run until the caller supplies an answer.
Ask `timeout: 0` waits without a deadline.
Set `on_fail: @skip` only when a node has a usable default.
Image and the other generation nodes dispatch to their named hub domain.
Generation dimensions and steps should respect the catalog range hints.
Flow `trigger:` is `@manual` or `@input`.
Flow `concurrency:` is a positive integer.
Flow `autostart: true` asks the server to create one instance at boot.
Flow `label:` is the human display name.
Flow `brief:` describes the service to tool callers.
Flow `tools:` may define named projections with `in` and `out` node ids.
The implicit `run` tool uses all Input and Output nodes.
The file is the source of truth; graph, tools and instances derive from it."#;
