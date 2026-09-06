# Named selections and geometric queries

Use these objects inside `model.apply.operations`, with the normal exact expected
head and request ID. Every operation supplies `object` and `name`. No viewport
state is used. At most128 named groups persist in a document; source/history
preserve them. IDs are canonical decimal strings. Queries inspect the editable
base mesh: make an instance unique or bake its modifiers before querying derived
geometry. Query positions explicitly choose local or world space.

`selection`: `vertices:[ID...]`, `faces:[ID...]` sets a group; both arrays required,
empty allowed. Every ID must exist; duplicates are normalized and sorted.

`select`: `element:"vertices"|"faces"`, `min:integer`, `max:integer`, `query:{...}`.
The selected element count must lie in the inclusive requested range. The result
and saved group include both vertex and face selections. Supported queries:

- `{kind:"all"}`: all base vertices and faces.
- `{kind:"material",material:integer}`: faces with that material and their vertices.
- `{kind:"bounds",min:XYZ,max:XYZ,space:"local"|"world"}`: vertices within the box;
  a face is included when every corner lies inside.
- `{kind:"connected",face:ID,stop_seams:boolean}`: edge-connected face component;
  optional UV-seam barriers. Includes those faces' vertices.
- `{kind:"loop"|"ring",edge:[vertexID,vertexID]}`: a quad ring crosses opposite
  face edges; a loop crosses valence-four vertices through nonadjacent edges and
  stops at poles/boundaries. Returns visited edge vertices and incident faces.
- `{kind:"ray",origin:XYZ,direction:XYZ,space:"local"|"world"}`: closest positive
  triangle hit, deterministic face-ID tie break. No hit is empty. Use min=1,max=1
  to require a hit. Direction must be nonzero; both face orientations are pickable.

`use_selection`: `element:"vertices"|"faces"|"face"`, `operation:{op,...}` fills the
selected field and object into one ordinary mesh operation. The template must
omit object and that selected field. `face` requires exactly one face; array
selections must be nonempty. Recursive selection/control/global operations are
refused. The entire surrounding transaction remains atomic and cancellable.

Example (plus document/request_id/expected envelope):

```json
{"operations":[
 {"op":"select","object":"body","name":"roof","element":"vertices","min":4,"max":64,
  "query":{"kind":"bounds","min":[-2,0.5,-2],"max":[2,3,2],"space":"local"}},
 {"op":"use_selection","object":"body","name":"roof","element":"vertices",
  "operation":{"op":"transform","matrix":[[1,0,0,0],[0,1,0,0.2],[0,0,1,0],[0,0,0,1]]}}
]}
```

Groups retain identity-based membership when positions/materials change. Losing
any selected element marks the group **stale**; subsequent use refuses it until
explicitly replaced or queried again. Deleting its object removes its groups.
`delete_selection` removes one named group. Inspect `selections`,
`selection_vertices`, and `selection_faces` with optional object and the normal
bounded offset/limit cursor. Operation outputs return the stable selections too.
