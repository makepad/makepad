// Port of box3d/src/sensor.h (+ sensor.c functions to be ported below the structs).

use crate::bitset::BitSet;

/// Used to track shapes that hit sensors using time of impact.
#[derive(Clone, Copy, Debug, Default)]
pub struct SensorHit {
    pub sensor_id: i32,
    pub visitor_id: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Visitor {
    pub shape_id: i32,
    pub generation: u16,
}

#[derive(Clone, Debug, Default)]
pub struct Sensor {
    pub hits: Vec<Visitor>,
    pub overlaps1: Vec<Visitor>,
    pub overlaps2: Vec<Visitor>,
    pub shape_id: i32,
}

#[derive(Clone, Debug, Default)]
pub struct SensorTaskContext {
    pub event_bits: BitSet,
}

// ---------------------------------------------------------------------------
// Port of sensor.c
// ---------------------------------------------------------------------------
//
// Sensor shapes need to
// - detect begin and end overlap events
// - events must be reported in deterministic order
// - maintain an active list of overlaps for query
//
// Assumption
// - sensors don't detect shapes on the same body
//
// Algorithm
// Query all sensors for overlaps
// Check against previous overlaps
//
// Data structures
// Each sensor has a double buffered array of overlaps
// These overlaps use a shape reference with index and generation
//
// Threading: the C code runs b3SensorTask as a parallel-for over sensors and
// merges the per-worker event bit sets in worker order. The port runs the same
// per-sensor work in one serial loop (identical order) with worker index 0.
// The custom filter callback is Option::take'n from the world for the duration
// of the queries so it can run while the world is borrowed immutably.

use crate::b3_assert;
use crate::bitset::{in_place_union, set_bit, set_bit_count_and_clear};
use crate::body::{get_body_transform, get_body_transform_quick};
use crate::container::array_remove_swap;
use crate::core::NULL_INDEX;
use crate::constants::MAX_SHAPE_CAST_POINTS;
use crate::ctz::ctz64;
use crate::dynamic_tree::dynamic_tree_query;
use crate::id::ShapeId;
use crate::math_functions::{inv_mul_transforms, min_int, to_relative_transform, transform_point, Transform, Vec3, POS_ZERO};
use crate::physics_world::{World, DISABLED_SET};
use crate::shape::{should_shapes_collide, Shape, ShapeGeometry};
use crate::types::{CustomFilterFcn, SensorBeginTouchEvent, SensorEndTouchEvent, ShapeProxy, ShapeType};

fn overlap_sensor(sensor_shape: &Shape, sensor_transform: Transform, visitor_shape: &Shape, visitor_transform: Transform) -> bool {
    let mut proxy_buffer = [Vec3::ZERO; 2];
    let proxy = crate::shape::make_shape_proxy(visitor_shape, &mut proxy_buffer);

    // Get the visitor shape in the frame of the sensor
    let relative_transform = inv_mul_transforms(sensor_transform, visitor_transform);

    let mut local_points = [Vec3::ZERO; MAX_SHAPE_CAST_POINTS];

    let local_count = min_int(proxy.count(), MAX_SHAPE_CAST_POINTS as i32);
    for i in 0..local_count {
        local_points[i as usize] = transform_point(relative_transform, proxy.points[i as usize]);
    }

    let local_proxy = ShapeProxy {
        points: &local_points[..local_count as usize],
        radius: proxy.radius,
    };

    match &sensor_shape.geom {
        ShapeGeometry::Capsule(capsule) => crate::capsule::overlap_capsule(capsule, Transform::IDENTITY, &local_proxy),

        ShapeGeometry::Compound(compound) => crate::compound::overlap_compound(compound, Transform::IDENTITY, &local_proxy),

        ShapeGeometry::HeightField(height_field) => {
            crate::height_field::overlap_height_field(height_field, Transform::IDENTITY, &local_proxy)
        }

        ShapeGeometry::Hull(hull) => crate::hull::overlap_hull(hull, Transform::IDENTITY, &local_proxy),

        ShapeGeometry::Mesh(mesh) => crate::mesh::overlap_mesh(mesh, Transform::IDENTITY, &local_proxy),

        ShapeGeometry::Sphere(sphere) => crate::sphere::overlap_sphere(sphere, Transform::IDENTITY, &local_proxy),
    }
}

/// C: b3SensorQueryCallback. The query context (world/sensor shape/transform)
/// is threaded through as parameters; overlaps are recorded into a local Vec
/// that is written back to the sensor after the queries.
fn sensor_query_callback(
    world: &World,
    custom_filter: &mut Option<Box<CustomFilterFcn>>,
    overlaps2: &mut Vec<Visitor>,
    sensor_shape_id: i32,
    transform: Transform,
    user_data: u64,
) -> bool {
    let shape_id = user_data as i32;

    if shape_id == sensor_shape_id {
        return true;
    }

    let sensor_shape = &world.shapes[sensor_shape_id as usize];
    let other_shape = &world.shapes[shape_id as usize];

    // Mesh vs mesh is not supported
    let other_type = other_shape.shape_type();
    let sensor_type = sensor_shape.shape_type();
    if (other_type == ShapeType::Mesh || other_type == ShapeType::Height)
        && (sensor_type == ShapeType::Mesh || sensor_type == ShapeType::Height)
    {
        return true;
    }

    // Are sensor events enabled on the other shape?
    if !other_shape.enable_sensor_events {
        return true;
    }

    // Skip shapes on the same body
    if other_shape.body_id == sensor_shape.body_id {
        return true;
    }

    // Check filter
    if !should_shapes_collide(sensor_shape.filter, other_shape.filter) {
        return true;
    }

    // Custom user filter
    if sensor_shape.enable_custom_filtering || other_shape.enable_custom_filtering {
        if let Some(custom_filter_fcn) = custom_filter.as_mut() {
            let id_a = ShapeId {
                index1: sensor_shape_id + 1,
                world0: world.world_id,
                generation: sensor_shape.generation,
            };
            let id_b = ShapeId {
                index1: shape_id + 1,
                world0: world.world_id,
                generation: other_shape.generation,
            };
            let should_collide = custom_filter_fcn(id_a, id_b);
            if !should_collide {
                return true;
            }
        }
    }

    let other_transform = to_relative_transform(get_body_transform(world, other_shape.body_id), POS_ZERO);

    let overlap = overlap_sensor(sensor_shape, transform, other_shape, other_transform);
    if !overlap {
        return true;
    }

    // Record the overlap
    overlaps2.push(Visitor {
        shape_id,
        generation: other_shape.generation,
    });

    true
}

/// C: b3SensorTask(startIndex, endIndex, workerIndex, context). The parallel-for
/// wrapper is folded into a direct serial call from overlap_sensors.
fn sensor_task(
    world: &mut World,
    start_index: i32,
    end_index: i32,
    worker_index: i32,
    custom_filter: &mut Option<Box<CustomFilterFcn>>,
) {
    b3_assert!(worker_index < world.worker_count);
    b3_assert!(start_index < end_index);

    for sensor_index in start_index..end_index {
        // Swap overlap arrays, append the sensor hits, clear the hits.
        let (sensor_shape_id, mut overlaps2) = {
            let sensor = &mut world.sensors[sensor_index as usize];

            // Swap overlap arrays
            std::mem::swap(&mut sensor.overlaps1, &mut sensor.overlaps2);
            sensor.overlaps2.clear();
            let mut overlaps2 = std::mem::take(&mut sensor.overlaps2);

            // Append sensor hits
            overlaps2.extend_from_slice(&sensor.hits);

            // Clear the hits
            sensor.hits.clear();

            (sensor.shape_id, overlaps2)
        };

        let (body_id, enable_sensor_events, mask_bits, query_bounds) = {
            let sensor_shape = &world.shapes[sensor_shape_id as usize];
            b3_assert!(sensor_shape.sensor_index == sensor_index);
            (
                sensor_shape.body_id,
                sensor_shape.enable_sensor_events,
                sensor_shape.filter.mask_bits,
                sensor_shape.aabb,
            )
        };

        let body_set_index = world.bodies[body_id as usize].set_index;
        if body_set_index == DISABLED_SET || !enable_sensor_events {
            let overlaps1_count = {
                let sensor = &mut world.sensors[sensor_index as usize];
                sensor.overlaps2 = overlaps2;
                sensor.overlaps1.len()
            };
            if overlaps1_count != 0 {
                // This sensor is dropping all overlaps because it has been disabled.
                set_bit(
                    &mut world.sensor_task_contexts[worker_index as usize].event_bits,
                    sensor_index as u32,
                );
            }
            continue;
        }

        let transform = to_relative_transform(get_body_transform_quick(world, body_id), POS_ZERO);

        // Query all trees
        {
            let world_ref: &World = world;
            let mut callback = |_proxy_id: i32, user_data: u64| -> bool {
                sensor_query_callback(world_ref, custom_filter, &mut overlaps2, sensor_shape_id, transform, user_data)
            };
            dynamic_tree_query(&world_ref.broad_phase.trees[0], query_bounds, mask_bits, false, &mut callback);
            dynamic_tree_query(&world_ref.broad_phase.trees[1], query_bounds, mask_bits, false, &mut callback);
            dynamic_tree_query(&world_ref.broad_phase.trees[2], query_bounds, mask_bits, false, &mut callback);
        }

        // Sort the overlaps to enable finding begin and end events.
        // C uses qsort with a shapeId comparator that never returns 0, so the
        // relative order of equal keys is unspecified there as well; the port
        // uses a total order on shapeId.
        overlaps2.sort_unstable_by(|a, b| a.shape_id.cmp(&b.shape_id));

        // Remove duplicates from overlaps2 (sorted). Duplicates are possible due
        // to the hit events appended earlier.
        let mut unique_count = 0usize;
        let overlap_count = overlaps2.len();
        for i in 0..overlap_count {
            if unique_count == 0 || overlaps2[i].shape_id != overlaps2[unique_count - 1].shape_id {
                overlaps2[unique_count] = overlaps2[i];
                unique_count += 1;
            }
        }
        overlaps2.truncate(unique_count);

        let something_changed = {
            let sensor = &world.sensors[sensor_index as usize];
            let count1 = sensor.overlaps1.len();
            let count2 = overlaps2.len();
            if count1 != count2 {
                // something changed
                true
            } else {
                let mut changed = false;
                for i in 0..count1 {
                    let s1 = &sensor.overlaps1[i];
                    let s2 = &overlaps2[i];

                    if s1.shape_id != s2.shape_id || s1.generation != s2.generation {
                        // something changed
                        changed = true;
                        break;
                    }
                }
                changed
            }
        };

        world.sensors[sensor_index as usize].overlaps2 = overlaps2;

        if something_changed {
            set_bit(
                &mut world.sensor_task_contexts[worker_index as usize].event_bits,
                sensor_index as u32,
            );
        }
    }
}

pub fn overlap_sensors(world: &mut World) {
    let sensor_count = world.sensors.len() as i32;
    if sensor_count == 0 {
        return;
    }

    b3_assert!(world.worker_count > 0);

    let worker_count = world.worker_count;
    for i in 0..worker_count as usize {
        set_bit_count_and_clear(&mut world.sensor_task_contexts[i].event_bits, sensor_count as u32);
    }

    // Parallel-for sensor overlaps.
    // C: b3ParallelFor(world, b3SensorTask, sensorCount, 16, world, "sensors")
    let mut custom_filter = world.custom_filter_fcn.take();
    sensor_task(world, 0, sensor_count, 0, &mut custom_filter);
    world.custom_filter_fcn = custom_filter;

    // Merge the per-worker event bits into worker 0 (no-op with one worker).
    {
        let (first, rest) = world.sensor_task_contexts.split_at_mut(1);
        for i in 1..worker_count as usize {
            in_place_union(&mut first[0].event_bits, &rest[i - 1].event_bits);
        }
    }

    // Iterate sensor bits and publish events.
    // Process sensor state changes. Iterate over set bits.
    let bit_set = std::mem::take(&mut world.sensor_task_contexts[0].event_bits);
    let block_count = bit_set.block_count;

    let world_id = world.world_id;
    let end_event_index = world.end_event_array_index as usize;

    for k in 0..block_count as usize {
        let mut word = bit_set.bits[k];
        while word != 0 {
            let ctz = ctz64(word);
            let sensor_index = (64 * k as u32 + ctz) as i32;

            {
                let World {
                    sensors,
                    shapes,
                    sensor_begin_events,
                    sensor_end_events,
                    ..
                } = &mut *world;

                let sensor = &sensors[sensor_index as usize];
                let sensor_shape = &shapes[sensor.shape_id as usize];
                let sensor_id = ShapeId {
                    index1: sensor.shape_id + 1,
                    world0: world_id,
                    generation: sensor_shape.generation,
                };

                let count1 = sensor.overlaps1.len();
                let count2 = sensor.overlaps2.len();
                let refs1 = &sensor.overlaps1;
                let refs2 = &sensor.overlaps2;

                // overlaps1 can have overlaps that end
                // overlaps2 can have overlaps that begin
                let mut index1 = 0usize;
                let mut index2 = 0usize;
                while index1 < count1 && index2 < count2 {
                    let r1 = &refs1[index1];
                    let r2 = &refs2[index2];
                    if r1.shape_id == r2.shape_id {
                        if r1.generation < r2.generation {
                            // end
                            let visitor_id = ShapeId { index1: r1.shape_id + 1, world0: world_id, generation: r1.generation };
                            sensor_end_events[end_event_index].push(SensorEndTouchEvent {
                                sensor_shape_id: sensor_id,
                                visitor_shape_id: visitor_id,
                            });
                            index1 += 1;
                        } else if r1.generation > r2.generation {
                            // begin
                            let visitor_id = ShapeId { index1: r2.shape_id + 1, world0: world_id, generation: r2.generation };
                            sensor_begin_events.push(SensorBeginTouchEvent {
                                sensor_shape_id: sensor_id,
                                visitor_shape_id: visitor_id,
                            });
                            index2 += 1;
                        } else {
                            // persisted
                            index1 += 1;
                            index2 += 1;
                        }
                    } else if r1.shape_id < r2.shape_id {
                        // end
                        let visitor_id = ShapeId { index1: r1.shape_id + 1, world0: world_id, generation: r1.generation };
                        sensor_end_events[end_event_index].push(SensorEndTouchEvent {
                            sensor_shape_id: sensor_id,
                            visitor_shape_id: visitor_id,
                        });
                        index1 += 1;
                    } else {
                        // begin
                        let visitor_id = ShapeId { index1: r2.shape_id + 1, world0: world_id, generation: r2.generation };
                        sensor_begin_events.push(SensorBeginTouchEvent {
                            sensor_shape_id: sensor_id,
                            visitor_shape_id: visitor_id,
                        });
                        index2 += 1;
                    }
                }

                while index1 < count1 {
                    // end
                    let r1 = &refs1[index1];
                    let visitor_id = ShapeId { index1: r1.shape_id + 1, world0: world_id, generation: r1.generation };
                    sensor_end_events[end_event_index].push(SensorEndTouchEvent {
                        sensor_shape_id: sensor_id,
                        visitor_shape_id: visitor_id,
                    });
                    index1 += 1;
                }

                while index2 < count2 {
                    // begin
                    let r2 = &refs2[index2];
                    let visitor_id = ShapeId { index1: r2.shape_id + 1, world0: world_id, generation: r2.generation };
                    sensor_begin_events.push(SensorBeginTouchEvent {
                        sensor_shape_id: sensor_id,
                        visitor_shape_id: visitor_id,
                    });
                    index2 += 1;
                }
            }

            // Clear the smallest set bit
            word &= word - 1;
        }
    }

    world.sensor_task_contexts[0].event_bits = bit_set;
}

/// C: b3DestroySensor(world, sensorShape). Takes the raw shape index.
pub fn destroy_sensor(world: &mut World, sensor_shape_id: i32) {
    let (sensor_index, shape_generation) = {
        let sensor_shape = &world.shapes[sensor_shape_id as usize];
        (sensor_shape.sensor_index, sensor_shape.generation)
    };

    let overlaps2 = std::mem::take(&mut world.sensors[sensor_index as usize].overlaps2);
    let end_event_index = world.end_event_array_index as usize;
    for visitor in &overlaps2 {
        let event = SensorEndTouchEvent {
            sensor_shape_id: ShapeId {
                index1: sensor_shape_id + 1,
                world0: world.world_id,
                generation: shape_generation,
            },
            visitor_shape_id: ShapeId {
                index1: visitor.shape_id + 1,
                world0: world.world_id,
                generation: visitor.generation,
            },
        };

        world.sensor_end_events[end_event_index].push(event);
    }

    // Destroy sensor
    {
        let sensor = &mut world.sensors[sensor_index as usize];
        sensor.hits = Vec::new();
        sensor.overlaps1 = Vec::new();
        // overlaps2 already taken above
    }

    let moved_index = array_remove_swap(&mut world.sensors, sensor_index);
    if moved_index != NULL_INDEX {
        // Fixup moved sensor
        let moved_shape_id = world.sensors[sensor_index as usize].shape_id;
        world.shapes[moved_shape_id as usize].sensor_index = sensor_index;
    }
}
