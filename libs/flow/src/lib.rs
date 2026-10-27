//! Makepad Flow: Splash flow documents, the run engine and its hosts.
//!
//! Feature layers (see `Cargo.toml`): `document` is the source, graph, value
//! and identity model; `runtime` adds the run scheduler, the shared flow
//! store and the bounded embedding engine, and links no widgets, platform,
//! HTTP, AI hub or video crate; `ai`, `http` and `media` add the provider
//! executors and the Publish node's executor over a result store
//! (`engine::results`); `host` and `client` are the HTTP planes and keep
//! every executor, as before.

#[cfg(feature = "document")]
pub(crate) mod digest;
#[cfg(feature = "document")]
pub mod document;
#[cfg(feature = "document")]
pub mod graph;
#[cfg(feature = "document")]
pub mod templates;
#[cfg(feature = "document")]
pub mod values;
#[cfg(feature = "document")]
pub mod wire;

#[cfg(feature = "runtime")]
pub mod engine;
#[cfg(feature = "runtime")]
pub mod instance;
#[cfg(feature = "runtime")]
pub mod triple;
#[cfg(all(feature = "runtime", not(target_arch = "wasm32")))]
pub mod runtime;
#[cfg(feature = "live")]
pub mod live;

#[cfg(all(feature = "client", not(target_arch = "wasm32")))]
pub mod client;
#[cfg(all(feature = "client", not(target_arch = "wasm32")))]
pub mod embed;

#[cfg(all(feature = "host", not(target_arch = "wasm32")))]
pub mod host;

#[cfg(feature = "runtime")]
pub use engine::*;
#[cfg(feature = "runtime")]
pub use instance::*;
#[cfg(feature = "document")]
pub use values::*;
#[cfg(feature = "document")]
pub use wire::*;

#[cfg(feature = "document")]
pub const PRELUDE: &str = include_str!("prelude.splash");

/// Compact authoring doctrine returned with the prelude catalog.
#[cfg(feature = "document")]
pub const AUTHORING_BRIEF: &str = r#"A flow file is ordinary splash and starts with `use mod.flow.*`.
Declare every node with one `let name = Type{...}` binding.
Finish the file with one `Flow{...}` expression.
List every node in that Flow object; its field name is the stable node id.
Node ids use splash identifiers and must be unique.
An input port is a field on the node.
Its value is a literal, another node's output reference, or for an ordered input a list of entries.
An ordered input takes `[conn(id, node.port()), ...]`: entries in order, each id a unique integer from 1.
An Fn declares an ordered input as `ordered(@type, bound, [entries])`; `[]` is no entries.
`Input{many: n}` and `Output{many: n}` make an ordered port of at most n values, at the root or as a component port.
An ordered Input's `value:` is a list of literals; a list literal on a one port stays one `list` value.
A one input reads one value and cannot read an ordered port.
The canvas adds, reorders and removes entries; `many`, an ordered port's `type` and its default list are source edits.
An output port is selected with `node.port()`.
Use `node.out(@port)` for custom output names or when an input shadows a named method.
Input nodes keep their design setting in `value:`; an instance or tool caller may override it for one run.
Output nodes name the results returned by the flow.
Set an Input or Output `type:` to a supported port type.
Builtin types are text, image, audio, video, mesh, json, list and bytes.
The host catalog also supplies registered node descriptors and qualified type keys.
Use a quoted key for a registered type: `type: "flow.text.lines"` or `ordered("flow.text.lines", 8, [...])`.
Registered node inputs, outputs and execution owners are fixed by that catalog; never invent types or node names.
Registered job artifacts travel as typed digest handles, including through Fn and component boundaries.
Local handles, audio ports and frame streams cannot be run by the job engine.
Canonical writes fail when a node or connection type is not registered by the host.
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
