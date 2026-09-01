use makepad_score::model::Id;

const HASH_LEFT: u64 = 0xcbf2_9ce4_8422_2325;
const HASH_RIGHT: u64 = 0x8422_2325_cbf2_9ce4;
const PRIME: u64 = 0x0000_0100_0000_01b3;

pub(crate) fn stable_id<K>(kind: &str, source_id: Option<&str>, path: &str) -> Id<K> {
    if let Some(id) = source_id.and_then(parse_exported_id) {
        return Id::new(id.0, id.1);
    }
    let identity = source_id.unwrap_or(path);
    let mut left = HASH_LEFT;
    let mut right = HASH_RIGHT;
    for byte in kind.bytes().chain([0]).chain(identity.bytes()) {
        left ^= u64::from(byte);
        left = left.wrapping_mul(PRIME);
        right ^= u64::from(byte.rotate_left(1));
        right = right.wrapping_mul(PRIME).rotate_left(7);
    }
    if left == 0 && right == 0 {
        right = 1;
    }
    Id::new(left, right)
}

pub(crate) fn score_id(identity: &str) -> [u8; 16] {
    let id = stable_id::<()>("score", None, identity);
    let mut bytes = [0_u8; 16];
    bytes[..8].copy_from_slice(&id.actor().to_le_bytes());
    bytes[8..].copy_from_slice(&id.counter().to_le_bytes());
    bytes
}

pub(crate) fn exported_id<K>(id: Id<K>) -> String {
    format!("mpid-{:016x}-{:016x}", id.actor(), id.counter())
}

fn parse_exported_id(value: &str) -> Option<(u64, u64)> {
    let value = value.strip_prefix("mpid-")?;
    let (actor, counter) = value.split_once('-')?;
    Some((
        u64::from_str_radix(actor, 16).ok()?,
        u64::from_str_radix(counter, 16).ok()?,
    ))
}
