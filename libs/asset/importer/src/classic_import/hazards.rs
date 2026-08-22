//! Damaging and liquid floors, exported as their own glTF nodes.
//!
//! A walker that cannot tell nukage from concrete drowns in it. Doom knows
//! exactly which sectors hurt (the sector special) and which are liquid (the
//! floor flat), so the exporter keeps that knowledge instead of melting it
//! into one anonymous level mesh: the hazard floor triangles move into nodes
//! named `hazard_N` carrying `{"kind":"hazard","damage":…,"flat":…,
//! "liquid":…,"solid":…}` in their `extras`.
//!
//! Sectors are grouped by (damage, flat), not one node per sector: a map's
//! twelve nukage pools are one `hazard_1`, and level collision tags every
//! triangle in it at once.

use std::collections::BTreeMap;

/// Percent of health Doom takes per 32 tics in a damaging sector.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Damage(pub u8);

/// Vanilla sector specials that hurt. Boom's generalized specials carry the
/// same three damage levels in bits 5–6, handled below.
pub(crate) fn vanilla_damage(special: u16) -> u8 {
    match special {
        // 5: nukage 10%, 7: slime 5%, 16: super damage 20%,
        // 4: 20% + blinking light, 11: 20% and ends the level.
        5 => 10,
        7 => 5,
        4 | 11 | 16 => 20,
        _ => 0,
    }
}

/// Boom generalized sector specials keep the vanilla special in bits 0–4 and
/// add damage in bits 5–6 (0 none, 1 5%, 2 10%, 3 20%).
pub(crate) fn damage_of_special(special: u16) -> u8 {
    if special < 32 {
        return vanilla_damage(special);
    }
    let boom = match (special >> 5) & 0x3 {
        1 => 5,
        2 => 10,
        3 => 20,
        _ => 0,
    };
    boom.max(vanilla_damage(special & 0x1F))
}

/// Flats that are liquid even where the sector does no damage (water), and
/// the damaging ones a mapper forgot to flag.
pub(crate) fn liquid_flat(flat: &str) -> bool {
    const LIQUIDS: &[&str] = &[
        "NUKAGE", "SLIME", "LAVA", "BLOOD", "SLUDGE", "FWATER", "WATER",
    ];
    let upper = flat.to_ascii_uppercase();
    LIQUIDS.iter().any(|l| upper.starts_with(l))
}

/// One exported hazard group: every floor triangle that shares this damage
/// level and flat.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HazardKind {
    /// `hazard_1`, `hazard_2`, … in a stable order (damage, then flat).
    pub name: String,
    pub damage: u8,
    pub flat: String,
    pub liquid: bool,
}

/// Which sectors are hazards, and which group each belongs to.
#[derive(Clone, Debug, Default)]
pub(crate) struct DoomHazards {
    pub kinds: Vec<HazardKind>,
    /// Sector index → position in `kinds`.
    pub by_sector: BTreeMap<usize, usize>,
}

impl DoomHazards {
    pub fn index_of(&self, sector: usize) -> Option<usize> {
        self.by_sector.get(&sector).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }
}

/// Find every damaging or liquid sector of a map.
pub(crate) fn detect_hazards(sectors: &[u8], sec_floor_tex: &[String]) -> DoomHazards {
    let n_sec = sectors.len() / 26;
    if n_sec == 0 {
        return DoomHazards::default();
    }
    // Group key → sector list, in a deterministic order.
    let mut groups: BTreeMap<(u8, String), Vec<usize>> = BTreeMap::new();
    for sec in 0..n_sec.min(sec_floor_tex.len()) {
        let special = u16_le(sectors, sec * 26 + 22);
        let damage = damage_of_special(special);
        let flat = sec_floor_tex[sec].to_ascii_uppercase();
        let liquid = liquid_flat(&flat);
        if damage == 0 && !liquid {
            continue;
        }
        groups.entry((damage, flat)).or_default().push(sec);
    }
    let mut kinds = Vec::new();
    let mut by_sector = BTreeMap::new();
    for ((damage, flat), sectors) in groups {
        let index = kinds.len();
        for sec in sectors {
            by_sector.insert(sec, index);
        }
        kinds.push(HazardKind {
            name: format!("hazard_{}", index + 1),
            damage,
            liquid: liquid_flat(&flat),
            flat,
        });
    }
    DoomHazards { kinds, by_sector }
}

/// Floor geometry of one hazard group, in level space (it does not move, it
/// is only tagged).
#[derive(Clone, Debug, Default)]
pub(crate) struct HazardFloor {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Baked sector light, same as the level mesh around it.
    pub colors: Vec<[f32; 3]>,
}

impl HazardFloor {
    pub fn is_empty(&self) -> bool {
        self.indices.len() < 3 || self.positions.is_empty()
    }
}

/// Where the flat emitters send a hazard sector's floor.
pub(crate) struct HazardSink<'a> {
    pub hazards: &'a DoomHazards,
    pub floors: &'a mut Vec<HazardFloor>,
}

fn u16_le(b: &[u8], o: usize) -> u16 {
    if o + 2 > b.len() {
        return 0;
    }
    u16::from_le_bytes([b[o], b[o + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sector(special: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0i16.to_le_bytes());
        out.extend_from_slice(&128i16.to_le_bytes());
        out.extend_from_slice(b"NUKAGE1\0");
        out.extend_from_slice(b"CEIL1\0\0\0");
        out.extend_from_slice(&0i16.to_le_bytes());
        out.extend_from_slice(&special.to_le_bytes());
        out.extend_from_slice(&0i16.to_le_bytes());
        out
    }

    #[test]
    fn vanilla_and_boom_damage_levels() {
        assert_eq!(damage_of_special(5), 10);
        assert_eq!(damage_of_special(7), 5);
        assert_eq!(damage_of_special(16), 20);
        assert_eq!(damage_of_special(11), 20);
        assert_eq!(damage_of_special(0), 0);
        assert_eq!(damage_of_special(9), 0, "secret is not damage");
        // Boom generalized: damage lives in bits 5-6.
        assert_eq!(damage_of_special(1 << 5), 5);
        assert_eq!(damage_of_special(2 << 5), 10);
        assert_eq!(damage_of_special(3 << 5), 20);
        // A generalized special whose damage bits are clear still hurts if
        // its vanilla base does (secret + nukage).
        assert_eq!(damage_of_special((1 << 7) | 5), 10);
    }

    #[test]
    fn liquid_flats_are_recognised_by_family() {
        assert!(liquid_flat("NUKAGE1"));
        assert!(liquid_flat("lava1"));
        assert!(liquid_flat("BLOOD3"));
        assert!(liquid_flat("FWATER1"));
        assert!(!liquid_flat("FLOOR0_1"));
    }

    #[test]
    fn sectors_group_by_damage_and_flat() {
        let mut sectors = Vec::new();
        sectors.extend_from_slice(&sector(5)); // nukage, 10%
        sectors.extend_from_slice(&sector(0)); // nukage flat, no damage
        sectors.extend_from_slice(&sector(5)); // nukage, 10% again
        let flats = vec!["NUKAGE1".to_string(), "NUKAGE1".into(), "NUKAGE1".into()];
        let h = detect_hazards(&sectors, &flats);
        assert_eq!(h.kinds.len(), 2, "{:?}", h.kinds);
        assert_eq!(h.index_of(0), h.index_of(2), "same damage + flat = one node");
        assert_ne!(h.index_of(0), h.index_of(1));
        let damaging = &h.kinds[h.index_of(0).unwrap()];
        assert_eq!(damaging.damage, 10);
        assert!(damaging.liquid);
        assert_eq!(damaging.flat, "NUKAGE1");
        assert_eq!(h.kinds[h.index_of(1).unwrap()].damage, 0);
    }

    #[test]
    fn a_plain_floor_is_not_a_hazard() {
        let mut sectors = Vec::new();
        sectors.extend_from_slice(&sector(0));
        let flats = vec!["FLOOR0_1".to_string()];
        assert!(detect_hazards(&sectors, &flats).is_empty());
    }
}
