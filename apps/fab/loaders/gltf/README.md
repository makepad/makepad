# Fab glTF loader

The in-tree reference loader builds an editable Fab `Document` from glTF or GLB.
Architectural metadata uses this minimal open vocabulary:

```json
{
  "node.extras": {
    "arch": {
      "kind": "wall|slab|roof|window|door|stair|column|beam|site|...",
      "level": 0,
      "level_name": "Ground Floor",
      "id": "stable source id",
      "material_class": "glass|wood|metal|concrete|plaster|..."
    },
    "props": {}
  },
  "scene.extras": {
    "arch": {
      "units": "m",
      "levels": [{ "index": 0, "name": "Ground Floor", "elevation": 0.0 }]
    }
  }
}
```

`arch.kind` is an open lower-case set. Scene metadata may also carry
`arch.north_deg` and `arch.site`; readers accept the older `elevation_m` level
field for compatibility, but writers use `elevation`.

Node extras use `{ "arch": { "kind", "priority", "level", "level_name", "id",
"material_class", "area_m2", "volume_m3" }, "props": { ... } }`; scene extras
carry `arch.units`, `north_deg`, `site`, and indexed levels with elevations. The
loader maps `arch.kind` to the element class and retains numeric `arch.priority`
as the render-order hint used only to resolve measured coplanar overlaps (higher
values win); geometry, picking, and measurement remain unchanged.

For architectural depth ties, node `arch.coplanar_priority` is a stable integer and the higher value wins only when faces are coplanar. Material extras may carry `arch.material_class` (including `glass`), alongside standard `KHR_materials_transmission` and `KHR_materials_ior`; `site` additionally accepts `city`, `timezone_min`, `summer_time`, `date_local` (`YYYY-MM-DDTHH:MM` when a full local calendar date exists), or the source-exact `day_of_year` and `minute_of_day` when no year was authored.

The optional `arch.site` object uses `lat`, `lon`, and `elevation_m`, plus `date` (`YYYY-MM-DD`),
`time` (`HH:MM` local civil time), `utc_offset_hours` (standard hours east of UTC),
and `dst` (adds one hour); the converter may also write `city`, `timezone_min`,
`summer_time`, `date_local`, `day_of_year` and `minute_of_day`, and the loader keeps
every present key as `arch.site.*` Fab document metadata while project north remains `arch.north_deg`.

