# Editable vehicles

Build the actual car geometry and materials with model operations, then bind
four separate tire objects. These operations use ordinary transactional
`model.apply`; the source document, undo history and published GLB preserve them.

```json
{"op":"vehicle_wheel","object":"front_left_tire",
 "connection":"wheel_front_left","pivot":[0,0,0],"radius":0.35,"width":0.22}
```

`delete_vehicle_wheel {object}` removes a binding without deleting its mesh.
`vehicle_wheel` replaces that object's existing binding. The four distinct
connections are `wheel_front_left`, `wheel_front_right`, `wheel_rear_left`,
and `wheel_rear_right`. An object can have one binding, with at most four per
document. Ordinary assets with no wheel bindings compile normally; a vehicle
with any bindings requires all four before compilation/publication.

Author in metres with Y up, the nose towards **+Z**, and the driver's left
towards **+X** (facing +Z, +X is on your left). Export reads the frame from
the wheels themselves: forward runs from the rear axle's middle to the front
axle's, so a car laid out along another axis is turned to face +Z, and a
left/right swap in the bindings is renamed by where each wheel stands. It
refuses only a layout with no frame: a front wheel not ahead of both rear
wheels, or an axle whose two wheels share a side. The vehicle runtime
rotates the model by 180 degrees around Y into its driving direction,
engine -Z.

Each wheel's rotation axle is **object-local X**. The cylinder primitive's
axle is Y, so rotate its geometry onto X using a mesh transform before binding.
For example, the row-major matrix
`[[0,-1,0,0],[1,0,0,0],[0,0,1,0],[0,0,0,1]]` rotates a cylinder onto X.
Place it with `object_node` translation and optionally parent it to the body.
Wheel node world transforms support translation and positive uniform scale;
rotated or nonuniform node bases are refused. Use geometry transforms for the
axle orientation. Wheel descendants may contain additional rigid rim geometry,
but cannot contain another wheel binding.

Build all four tires the same way: geometry centred on the hub with `pivot`
`[0,0,0]` at the node origin, and the right side a translated copy of the
left, never mirrored (negative scale or a mirror op) or turned 180 degrees.
At runtime each tire rolls about the model's X axis through its hub, forward
when the car moves forward, whatever its geometry's own orientation.

`pivot` is the wheel centre in object-local coordinates. Export computes its
model-space anchor through the object hierarchy. `radius` and `width` are
positive model-space metre measurements, at most 10,000; update them after
scaling geometry. Pivot edits preserve the anchor. The wheel object must have
exported faces. Skin skeletons and driven wheel bindings cannot be combined.
Geometry remains editable; tire UVs and material layers use the ordinary paths.

Export writes the runtime contract directly on the wheel's glTF node:
`extras: {kind:"vehicle_wheel",connection,pivot,anchor,radius,width}`. The
renderer separates that geometry from the static body stream. Sandbox fits its
four suspension probes to the declared anchors/radii, then renders steering,
spin, suspension travel and detached wheels through the same binding.
A generic named `socket` is an attachment marker and does not animate a tire.

For headlights, use actual `light` operations attached to the body. Place them
near the +Z nose. A spot emits along its local -Z, so use rotation `[0,1,0,0]`
to aim it towards the car's +Z front. Example:

```json
{"op":"light","name":"headlight_left","attachment":{"object":"body"},
 "transform":{"translation":[0.6,0.15,2.0],"rotation":[0,1,0,0]},
 "kind":"spot","inner":0.12,"outer":0.4,
 "color":[1,0.9,0.7],"intensity":150,"range":30}
```

Intensity is candela, range metres, cone half-angles radians. The full car
instance transform places both mesh and emitters; model scale does not change
light intensity or range. Use an emissive material for a visibly glowing lamp
surface as well. For a daylight game, start the lens around emissive strength
0.1 and compare its highlights and beam against the sun before increasing it.
Keep lens color, metallic and roughness explicit when using `surface_material`:
omitted factors reset to defaults, including emissive RGB zero.
Authored lights remain independent of the gameplay command
that toggles generic entity headlights. The host automatically suppresses its
two generic headlights when the installed exterior has authored emitters;
independent script lights remain available.

Publish the finished asset and use its returned reference for the actual car
spawn, with explicit chassis dimensions matching the authored exterior length.
The chassis remains the physics body; model length determines the common
visual/wheel scale. Seats use the existing vehicle seat count/placement.
Named seat sockets and door meshes can be authored, but do not automatically
replace gameplay seat placement or supply an opening-door interaction.

## Visual steering and suspension

`vehicle_wheel` accepts optional `visual:{steer_gain:0.55,steer_max:0.32,compression:0.08,droop:0.10}`. All fields are required together: gain 0..1, angle cap 0..1.2 radians, compression/droop 0..5 model metres. Display steering is clamp(physical steering*gain, -cap, cap); display suspension is clamped to -droop..compression. This affects only visible geometry. Turning radius, tire forces, physical suspension, anchors, radii and spin are unchanged. Without visual metadata, existing motion and source serialization remain unchanged.

The authored limits travel with the shared asset and are used by both the game renderer and model.render. Preserve the intended body silhouette, select an appropriate display range, and verify real clearance within that range. Do not grow giant fenders solely to pass the legacy diagnostic pose.

## Publishing as a vehicle

Binding the four wheels is what makes the published model a VEHICLE: it
publishes with kind vehicle and the `vehicle` + type tag (car/truck/bus/van/
kart), shows under Vehicles and "car" in the Explorer, and drops into any
game as a drivable car (E gets in and out). Name tyre objects by corner too
(`front_left_tire`, `rear-right-tire`, `wheel_fl`): unbound but so named, they
are bound at publish. Put "car", "truck", "van", "bus" or "kart" in the title.
Optional: `seat_1` (or `seat`) socket at the driver's hips, `seat_2`,
`seat_3`… at the passengers' (without them the seats are derived from the
car's size), and a `steering_wheel` pivot.
