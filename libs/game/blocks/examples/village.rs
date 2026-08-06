//! Villager cost + behaviour probe: `cargo run --release --example village`.
//!
//! Prints per-tick cost at two crowd sizes and a readable trace of what a
//! handful of villagers actually did, since behaviour has no other inspection
//! surface without a screen.

use makepad_game_blocks::*;
use makepad_game_sim::*;
use makepad_math::*;
use std::time::Instant;

fn ground(world: &mut GameWorld) {
    world.next_id += 1;
    let id = world.next_id;
    world.push_entity(Entity {
        id,
        kind: BodyKind::Static,
        pos: vec3f(0.0, -1.0, 0.0),
        half: vec3f(200.0, 1.0, 200.0),
        collide: true,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        density: 1.0,
        friction: 0.6,
        ..Default::default()
    });
}

fn prop(world: &mut GameWorld, pos: Vec3f, half: Vec3f) {
    world.next_id += 1;
    let id = world.next_id;
    world.push_entity(Entity {
        id,
        kind: BodyKind::Static,
        pos,
        half,
        collide: true,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        density: 1.0,
        friction: 0.6,
        ..Default::default()
    });
}

fn village(count: usize) -> (GameWorld, Blocks) {
    let mut world = GameWorld::new();
    world.reset_content();
    world.gravity = 30.0;
    ground(&mut world);
    // Some buildings to walk around.
    for i in 0..12 {
        let a = i as f32 * 0.52;
        prop(
            &mut world,
            vec3f(a.cos() * 26.0, 2.0, a.sin() * 26.0),
            vec3f(3.0, 2.0, 3.0),
        );
    }
    let mut blocks = Blocks::new();
    for (x, z, tag) in [
        (10.0, 4.0, "bench"),
        (-12.0, 7.0, "well"),
        (5.0, -15.0, "door"),
        (-8.0, -12.0, "market"),
        (16.0, -6.0, "lamp"),
        (0.0, 18.0, "work"),
    ] {
        blocks
            .pois
            .push(Poi::new(vec3f(x, 0.0, z), tag).with_capacity(2));
    }
    for i in 0..count {
        let a = i as f32 * 0.7;
        let r = 6.0 + (i % 7) as f32 * 1.5;
        let pos = vec3f(a.cos() * r, 1.0, a.sin() * r);
        world.next_id += 1;
        let id = world.next_id;
        world.push_entity(Entity {
            id,
            kind: BodyKind::Mover,
            pos,
            half: vec3f(0.4, 0.8, 0.4),
            collide: true,
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            gravity_scale: 1.0,
            speed_mult: 1.0,
            turn_rate: 9.0,
            auto_face: true,
            density: 1.0,
            ..Default::default()
        });
        blocks
            .npcs
            .push(Npc::new(id, NpcConfig::default(), pos, 500 + i as u64));
    }
    (world, blocks)
}

fn bench(count: usize, ticks: usize) {
    let (mut world, mut blocks) = village(count);
    // Warm the caches so the first tick's allocation isn't the headline.
    for _ in 0..60 {
        blocks.pre_step(&mut world);
        step_world(&mut world);
        blocks.post_step(&mut world);
    }
    let t = Instant::now();
    for _ in 0..ticks {
        blocks.pre_step(&mut world);
        step_world(&mut world);
        blocks.post_step(&mut world);
    }
    let per_tick = t.elapsed().as_secs_f64() * 1000.0 / ticks as f64;
    // Isolate the NPC share by running the same world with the blocks phase only.
    let t = Instant::now();
    for _ in 0..ticks {
        blocks.pre_step(&mut world);
    }
    let npc_only = t.elapsed().as_secs_f64() * 1000.0 / ticks as f64;
    println!(
        "{count:>4} npcs: {per_tick:.3} ms/tick full  ({npc_only:.3} ms/tick in blocks::pre_step)"
    );
}

fn main() {
    bench(50, 600);
    bench(200, 600);

    println!("\n--- a few villagers over four minutes ---");
    let (mut world, mut blocks) = village(6);
    for t in 0..60 * 240 {
        blocks.pre_step(&mut world);
        step_world(&mut world);
        blocks.post_step(&mut world);
        if t % (60 * 20) == 0 {
            println!("[t={:>4}s]", t / 60);
            for n in blocks.npcs.iter().take(3) {
                println!("  {}", n.trace(&world));
            }
        }
    }
}
