use crate::rigid_body::{BodyType, RigidBody};

/// FNV-1a 64-bit hash offset basis.
const FNV_OFFSET: u64 = 14695981039346656037;
/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 1099511628211;

/// Deterministic FNV-1a hash of the full physics state.
///
/// Hashes position, rotation, linear velocity, angular velocity, and
/// sleep/body-type state of every body as raw bytes.
/// Deterministic because:
/// - Bodies are in a Vec with stable insertion order
/// - All f32 values are produced by deterministic IEEE 754 arithmetic
/// - Byte order is fixed (to_le_bytes)
pub fn hash_bodies(bodies: &[RigidBody]) -> u64 {
    let mut h: u64 = FNV_OFFSET;
    for body in bodies {
        hash_f32(&mut h, body.pose.position.x);
        hash_f32(&mut h, body.pose.position.y);
        hash_f32(&mut h, body.pose.position.z);
        hash_f32(&mut h, body.pose.orientation.x);
        hash_f32(&mut h, body.pose.orientation.y);
        hash_f32(&mut h, body.pose.orientation.z);
        hash_f32(&mut h, body.pose.orientation.w);
        hash_f32(&mut h, body.linear_velocity.x);
        hash_f32(&mut h, body.linear_velocity.y);
        hash_f32(&mut h, body.linear_velocity.z);
        hash_f32(&mut h, body.angular_velocity.x);
        hash_f32(&mut h, body.angular_velocity.y);
        hash_f32(&mut h, body.angular_velocity.z);
        hash_f32(&mut h, body.sleep_time);
        hash_u8(&mut h, if body.sleeping { 1 } else { 0 });
        hash_u8(
            &mut h,
            match body.body_type {
                BodyType::Dynamic => 1,
                BodyType::Fixed => 0,
            },
        );
    }
    h
}

#[inline]
fn hash_f32(h: &mut u64, v: f32) {
    for byte in v.to_bits().to_le_bytes() {
        *h ^= byte as u64;
        *h = h.wrapping_mul(FNV_PRIME);
    }
}

#[inline]
fn hash_u8(h: &mut u64, v: u8) {
    *h ^= v as u64;
    *h = h.wrapping_mul(FNV_PRIME);
}
