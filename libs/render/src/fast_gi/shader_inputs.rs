//! A sleeping field must wake for every external input read by its producers.
use super::*;
use makepad_platform::makepad_script::{
    script_eval, shader_ir::{expr_children, ExprId, ExprKind, IrGlobalKind, IrIo, IrTy, Stmt, NO_EXPR},
    shader_ir_draw::{compile_draw_shader_ir_with, IrCompileOptions},
    shader_output::{ShaderOutput, TextureType},
};
use std::collections::BTreeSet;

// Exercise the production key's equality for each shader binding it covers.
// Mixed transport/dispatch uniforms are checked by component.
fn keyed_inputs(differs: impl Fn(&TransportKey, TransportKey) -> bool) -> BTreeSet<&'static str> {
    let base = || TransportKey { shadow_filter: Some((false, 0)), ..Default::default() };
    let held = base();
    let mut covered = BTreeSet::new();
    let mut field = |names: &[&'static str], change: fn(&mut TransportKey)| {
        let mut candidate = base();
        change(&mut candidate);
        if differs(&held, candidate) { covered.extend(names.iter().copied()); }
    };
    field(&["uniform gi_sun_dir"], |k| k.sun_and_sky[0][0] = 1);
    field(&["uniform gi_sun_color"], |k| k.sun_and_sky[1][0] = 1);
    field(&["uniform gi_sky"], |k| k.sun_and_sky[2][0] = 1);
    field(&["uniform gi_ground"], |k| k.sun_and_sky[3][0] = 1);
    // cluster_data holds the selected lamp texels and the shadow records
    // addressed by local_shadow_tex. The latter follow the selected face wake.
    // The ids address rows held in the key, so renumbering equal rows preserves the producer input.
    field(&["uniform gi_light_ids0", "uniform gi_light_ids1", "texture cluster_data"], |k| k.lamp_texels[0][0] = 1);
    field(&["uniform local_shadow_soft"], |k| k.shadow_filter = Some((true, 0)));
    field(&["uniform local_shadow_source_radius"], |k| k.shadow_filter = Some((false, 1)));
    field(&["uniform gi_relight.x"], |k| k.feedback = 1);
    field(&["uniform gi_relight.y"], |k| k.linear_albedo = true);
    field(&["uniform gi_relight.z"], |k| k.emission_gain = 1);
    field(&["uniform gi_relight.w", "uniform gi_pass.z"], |k| k.ray_distance = 1);
    field(&["uniform gi_bounce"], |k| k.bounce = 1);
    field(&["live literal"], |k| k.scope_uniforms_gen[0] = 1);
    covered
}

// Each remaining input either has its own publication/wake or carries internal
// data for the current issue. Raster transforms apply only in the vertex stage.
fn covered_elsewhere(shader: LiveId, stage: &str, input: &str) -> bool {
    if stage == "io_vertex" && matches!(input,
        "geometry geom.pos" |          // Unit quad corners.
        "instance rect_pos" |          // Origin of the row or atlas tile.
        "instance rect_size" |         // Extent of the row or atlas tile.
        "instance draw_clip" |         // Clip of the offscreen target.
        "instance draw_depth" |        // Fixed quad depth.
        "buffer draw_call.zbias" |     // Orders raster draws.
        "buffer draw_list.view_shift" | // Offscreen draw-list translation.
        "buffer draw_list.view_clip" | // Offscreen draw-list clip.
        "buffer draw_list.view_transform" | // Places the producer quad.
        "pass CameraProjection" |      // Target-sized orthographic projection.
        "pass CameraView" |            // Offscreen pass view.
        "vertex position"              // Clip-space output.
    ) { return true; }
    let fragment = stage == "io_fragment";
    if matches!(input,
        "texture local_shadow_map" |   // Selected faces wake on content or camera changes. Tiles follow the current metadata.
        "depth local_shadow_map" |     // The depth-compare variant uses the same cache decision.
        "uniform local_shadow_on" |    // A selected lamp always has an active face record.
        "uniform local_shadow_tex.x" | // Offset of the selected shadow metadata in cluster_data.
        "uniform local_shadow_tex.z" | // Atlas texel width follows shadow allocation/invalidation.
        "uniform local_shadow_tex.w" | // Atlas texel height follows shadow allocation/invalidation.
        "uniform cluster_z.w" |        // Offset of the compared lamp records in cluster_data.
        "uniform cluster_tex.x" |      // Converts a logical record to a column in the packed table.
        "uniform cluster_tex.y" |      // Converts a logical record to a row in the packed table.
        "uniform gi_on" |              // Relight binds one for bounce sampling.
        "uniform gi_fallback_floor" |  // Producer fallback is fixed at zero.
        "uniform gi_back_cut" |        // Producer back-face cutoff is fixed at zero.
        "uniform gi_debug"             // Producer debug mode is fixed off.
    ) { return fragment && shader == id!(DrawGiRelight); }
    match input {
        // Relight and gather consume the trace result and the previous iterate.
        "texture gi_hits" | "texture gi_field" => return fragment && matches!(shader, id!(DrawGiRelight) | id!(DrawGiGather)),
        "texture gi_radiance" => return fragment && shader == id!(DrawGiGather), // This issue's relight result.
        "texture gi_update" => return fragment && shader == id!(DrawGiScatter), // This issue's gathered tiles.
        "fragment position" => return fragment && shader != id!(DrawGiScatter), // Producer target texel.
        "instance gi_tile" => return shader == id!(DrawGiScatter), // Destination row and region, or slot clear.
        _ => {}
    }
    matches!(input,
        "uniform gi_grid" |            // A grid layout change releases the field.
        "uniform gi_home" |            // A probe home or cascade count change releases the field.
        "uniform gi_atlas" |           // Field tile layout follows that allocation.
        // A window commit wakes exposed slots. Until their next restart, settled
        // kept probes retain the value traced under the earlier windows, including
        // the bounce read through the cascade blend at the hit.
        "uniform gi_c0" |
        "uniform gi_c1" |
        "uniform gi_c2" |
        "uniform gi_vox0" |
        "uniform gi_vox1" |
        "uniform gi_vox2" |
        "uniform gi_vox_dims" |        // Fixed by the voxel allocation.
        "uniform gi_vox_tex" |         // Addresses the allocation's packed levels.
        "uniform gi_scene_min" |       // Published lower bound of the voxel scene.
        "uniform gi_scene_max" |       // Published upper bound of the voxel scene.
        "texture gi_vox_a" |           // Occupancy and albedo wake on voxel publication.
        "texture gi_vox_b" |           // Normals wake on voxel publication.
        "texture gi_vox_c" |           // Emission wakes on voxel publication.
        "texture gi_skyline" |         // The skyline is published with the changed scene.
        "uniform gi_skyline_size" |    // Dimensions of that published skyline.
        "uniform gi_skyline_rect" |    // World extent of that published skyline.
        "texture gi_batch" |           // Destinations, world cells and history of the scheduler's current issues.
        "uniform gi_batch_height" |    // Allocated height of the dense scheduler list.
        "uniform gi_pass.x" |          // This issue's band height.
        "uniform gi_pass.y" |          // Physical height of the three batch targets.
        "uniform gi_pass.w" |          // Rotates the noise samples within the finite countdown.
        "varying pos" |                // Position interpolated from the producer quad.
        "varying world" |              // Offscreen position produced by its vertex function.
        "output fb0" |                 // The pass's colour destination.
        "loop counter"                 // Compiler-owned bound on shader iterations.
    )
}

// Start at statement operands. The compiler also keeps unused expressions and
// inherited declarations; only a value reached from an entry's body is a read.
fn roots(body: &[Stmt], out: &mut Vec<ExprId>) {
    for stmt in body {
        match stmt {
            Stmt::Local(_, e) | Stmt::Expr(e) | Stmt::Return(e) => out.push(*e),
            Stmt::Assign(a, b) | Stmt::CompoundAssign(a, _, b) => out.extend([*a, *b]),
            Stmt::If(e, yes, no) => { out.push(*e); roots(yes, out); roots(no, out); }
            Stmt::For(_, a, b, body) => { out.extend([*a, *b]); roots(body, out); }
            Stmt::Loop(body) => roots(body, out),
            Stmt::Break | Stmt::Continue | Stmt::Discard => {}
        }
    }
}

fn reads(output: &ShaderOutput, stage: &str) -> BTreeSet<String> {
    let ir = &output.ir;
    let root = ir.function_by_name(stage).expect("GI shader entry is present");
    let mut inputs = BTreeSet::new();
    for index in ir.reachable(root) {
        let f = &ir.functions[index as usize];
        let mut todo = Vec::new();
        roots(&f.body, &mut todo);
        let mut seen = vec![false; f.exprs.len()];
        let global = |base: ExprId| match f.exprs[base as usize].kind {
            ExprKind::Global(g) => Some(&ir.globals[g as usize]),
            _ => None,
        };
        while let Some(index) = todo.pop() {
            if index == NO_EXPR || std::mem::replace(&mut seen[index as usize], true) { continue; }
            let expr = &f.exprs[index as usize].kind;
            match *expr {
                ExprKind::Field(base, member) => {
                    if let Some(g) = global(base) {
                        let kind = match g.kind {
                            IrGlobalKind::Io(IrIo::Geometry) => Some("geometry"),
                            IrGlobalKind::Io(IrIo::UniformBuffer) => Some("buffer"),
                            _ => None,
                        };
                        if let (Some(kind), IrTy::Struct(s)) = (kind, ir.get(g.ty)) {
                            let name = ir.structs[*s as usize].fields[member as usize].name;
                            inputs.insert(format!("{kind} {}.{name}", g.name));
                            continue;
                        }
                    }
                }
                ExprKind::Swizzle(base, lanes, n) => {
                    if let Some(g) = global(base) {
                        if g.kind == IrGlobalKind::Io(IrIo::Uniform) && matches!(g.name,
                            id!(gi_pass) | id!(gi_relight) | id!(cluster_z) | id!(cluster_tex) | id!(local_shadow_tex)) {
                            for &lane in &lanes[..n as usize] {
                                inputs.insert(format!("uniform {}.{}", g.name, b"xyzw"[lane as usize] as char));
                            }
                            continue;
                        }
                    }
                }
                ExprKind::Global(g) => {
                    let g = &ir.globals[g as usize];
                    let kind = match g.kind {
                        IrGlobalKind::Io(IrIo::Uniform) => "uniform",
                        IrGlobalKind::Io(IrIo::RustInstance | IrIo::DynInstance) => "instance",
                        IrGlobalKind::Io(IrIo::Texture(TextureType::Texture2d)) => "texture",
                        IrGlobalKind::Io(IrIo::Texture(TextureType::TextureDepth)) => "depth",
                        IrGlobalKind::Io(IrIo::Varying) => "varying",
                        IrGlobalKind::Io(IrIo::FragmentPosition) => { inputs.insert("fragment position".into()); continue; }
                        IrGlobalKind::Io(IrIo::FragmentOutput(0)) => "output",
                        IrGlobalKind::Io(IrIo::ScopeUniform) if output.table_consts().iter().any(|c| c.shader_name == g.name) => {
                            inputs.insert("live literal".into()); continue;
                        }
                        IrGlobalKind::IterCounter => { inputs.insert("loop counter".into()); continue; }
                        IrGlobalKind::VertexPosition => { inputs.insert("vertex position".into()); continue; }
                        other => { inputs.insert(format!("{other:?} {}", g.name)); continue; }
                    };
                    inputs.insert(format!("{kind} {}", g.name));
                }
                ExprKind::PassMatrix(matrix) => { inputs.insert(format!("pass {matrix:?}")); }
                ExprKind::InstanceIndex | ExprKind::InstanceTexWord(_) | ExprKind::DepthClip(..) |
                ExprKind::EntryIn(_) | ExprKind::EntryOut(_) => { inputs.insert(format!("{expr:?}")); }
                _ => {}
            }
            expr_children(expr, &mut todo);
        }
    }
    inputs
}

fn missing(inputs: &BTreeSet<String>, shader: LiveId, stage: &str, keys: &BTreeSet<&str>) -> Vec<String> {
    inputs.iter().filter(|s| !keys.contains(s.as_str()) && !covered_elsewhere(shader, stage, s)).cloned().collect()
}

#[test]
fn producer_reads_are_covered_by_a_wake_or_a_named_internal_input() {
    let keys = keyed_inputs(|held, candidate| *held != candidate);
    let without_sun = keyed_inputs(|held, mut candidate| {
        candidate.sun_and_sky[0] = held.sun_and_sky[0];
        *held != candidate
    });
    for hardware in [false, true] {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            vm.capture_errors();
            tests::register_gi_producers(vm, hardware);
            let draw = vm.value(vm.modules(), id!(draw)).as_object().expect("mod.draw");
            let opts = IrCompileOptions { const_table: true, ..Default::default() };
            for name in [id!(DrawGiTrace), id!(DrawGiRelight), id!(DrawGiGather), id!(DrawGiScatter)] {
                let obj = vm.value(draw, name).as_object().expect("composed producer");
                let output = compile_draw_shader_ir_with(vm, obj, &opts);
                assert!(!output.has_errors, "{name}, hardware={hardware}: {}", output.error_report());
                assert!(!output.uses_time, "{name} now depends on the pass clock");
                for stage in ["io_vertex", "io_fragment"] {
                    let inputs = reads(&output, stage);
                    let extra = missing(&inputs, name, stage, &keys);
                    assert!(extra.is_empty(), "{name}/{stage}, hardware={hardware}: unclassified {extra:?}");
                    if name == id!(DrawGiRelight) && stage == "io_fragment" {
                        let map = if hardware { "depth local_shadow_map" } else { "texture local_shadow_map" };
                        assert!(inputs.contains(map), "relight must reach the configured shadow sampler {map}");
                        assert_eq!(missing(&inputs, name, stage, &without_sun), ["uniform gi_sun_dir"],
                            "dropping sun direction from key equality must expose its shader read");
                        assert!(!inputs.contains("uniform cluster_on"), "the inherited cluster switch is unused by relight");
                    }
                }
            }
            if !hardware {
                // Three independent input mutations in one helper reached through the inherited fragment.
                let mutant = script_eval!(vm, {
                    use mod.prelude.widgets_internal.*
                    mod.draw.DrawGiRelight {
                        guard_gain: uniform(1.0)
                        guard_tex: texture_2d(float)
                        guard_value: fn() { return self.guard_tex.sample_nearest(self.pos) * (self.guard_gain + self.cluster_on) }
                        pixel: fn() { return self.guard_value() }
                    }
                }).as_object().expect("mutated producer");
                let output = compile_draw_shader_ir_with(vm, mutant, &opts);
                assert!(!output.has_errors, "input mutations must compile: {}", output.error_report());
                let extra = missing(&reads(&output, "io_fragment"), id!(DrawGiRelight), "io_fragment", &keys);
                assert_eq!(extra, ["texture guard_tex", "uniform cluster_on", "uniform guard_gain"],
                    "the guard must reject the added texture, newly read inherited uniform and added uniform");
            }
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "GI input guard setup or compilation failed: {errors:?}");
        });
    }
}
