use super::ini::Ini;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    pub units: BTreeMap<String, UnitRules>,
    pub weapons: BTreeMap<String, WeaponRules>,
    pub warheads: BTreeMap<String, WarheadRules>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnitRules {
    pub cost: i32,
    pub strength: i32,
    pub speed: i32,
    pub armor: String,
    pub primary: String,
    pub secondary: String,
    pub sight: i32,
    pub prerequisite: Vec<String>,
    pub owner: Vec<String>,
    pub tech_level: i32,
    pub points: i32,
    pub image: String,
    pub foundation: String,
    pub explodes: bool,
    pub harvester: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WeaponRules {
    pub damage: i32,
    pub rof: i32,
    pub range: f32,
    pub projectile: String,
    pub warhead: String,
    pub report: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WarheadRules {
    /// Percentages for none, wood, light, heavy, and concrete armor.
    pub verses: [u16; 5],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RulesError {
    InvalidNumber { section: String, key: &'static str },
    InvalidBoolean { section: String, key: &'static str },
    InvalidVerses { section: String },
}

impl fmt::Display for RulesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNumber { section, key } => {
                write!(f, "invalid number in [{section}] {key}")
            }
            Self::InvalidBoolean { section, key } => {
                write!(f, "invalid boolean in [{section}] {key}")
            }
            Self::InvalidVerses { section } => write!(f, "invalid [{section}] Verses"),
        }
    }
}

impl std::error::Error for RulesError {}

impl Rules {
    pub fn parse(ini: &Ini) -> Result<Self, RulesError> {
        let unit_names = unit_names(ini);
        let mut rules = Self::default();
        for (section, values) in ini.sections() {
            if unit_names.contains(&section.to_ascii_uppercase()) {
                rules
                    .units
                    .insert(section.to_owned(), parse_unit(section, ini)?);
                continue;
            }
            if has_key(values, "Verses") {
                rules
                    .warheads
                    .insert(section.to_owned(), parse_warhead(section, ini)?);
                continue;
            }
            if has_key(values, "Damage")
                && (has_key(values, "Warhead") || has_key(values, "Projectile"))
            {
                rules
                    .weapons
                    .insert(section.to_owned(), parse_weapon(section, ini)?);
            }
        }
        Ok(rules)
    }

    /// Parses RULES.INI and overlays the presentation fields supplied by
    /// ART.INI sections with matching unit names.
    pub fn parse_with_art(rules_ini: &Ini, art_ini: &Ini) -> Result<Self, RulesError> {
        let mut rules = Self::parse(rules_ini)?;
        rules.apply_art(art_ini);
        Ok(rules)
    }

    pub fn parse_ts(rules_ini: &Ini, art_ini: &Ini) -> Result<Self, RulesError> {
        Self::parse_with_art(rules_ini, art_ini)
    }

    pub fn apply_art(&mut self, art_ini: &Ini) {
        for (name, unit) in &mut self.units {
            if let Some(image) = art_ini.get(name, "Image") {
                unit.image = image.trim().to_owned();
            }
            if let Some(foundation) = art_ini.get(name, "Foundation") {
                unit.foundation = foundation.trim().to_owned();
            }
        }
    }
}

fn unit_names(ini: &Ini) -> BTreeSet<String> {
    [
        "InfantryTypes",
        "VehicleTypes",
        "AircraftTypes",
        "VesselTypes",
        "BuildingTypes",
        "UnitTypes",
    ]
    .into_iter()
    .flat_map(|section| {
        ini.section(section)
            .unwrap_or_default()
            .iter()
            .map(|(_, value)| value.trim().to_ascii_uppercase())
    })
    .collect()
}

fn parse_unit(section: &str, ini: &Ini) -> Result<UnitRules, RulesError> {
    Ok(UnitRules {
        cost: number(ini, section, "Cost")?,
        strength: number(ini, section, "Strength")?,
        speed: number(ini, section, "Speed")?,
        armor: text(ini, section, "Armor"),
        primary: text(ini, section, "Primary"),
        secondary: text(ini, section, "Secondary"),
        sight: number(ini, section, "Sight")?,
        prerequisite: list(ini, section, "Prerequisite"),
        owner: list(ini, section, "Owner"),
        tech_level: number(ini, section, "TechLevel")?,
        points: number(ini, section, "Points")?,
        image: text(ini, section, "Image"),
        foundation: text(ini, section, "Foundation"),
        explodes: boolean(ini, section, "Explodes")?,
        harvester: boolean_value(
            ini.get(section, "Harvester?")
                .or_else(|| ini.get(section, "Harvester")),
            section,
            "Harvester?",
        )?,
    })
}

fn parse_weapon(section: &str, ini: &Ini) -> Result<WeaponRules, RulesError> {
    Ok(WeaponRules {
        damage: number(ini, section, "Damage")?,
        rof: number(ini, section, "ROF")?,
        range: decimal(ini, section, "Range")?,
        projectile: text(ini, section, "Projectile"),
        warhead: text(ini, section, "Warhead"),
        report: list(ini, section, "Report"),
    })
}

fn parse_warhead(section: &str, ini: &Ini) -> Result<WarheadRules, RulesError> {
    let Some(value) = ini.get(section, "Verses") else {
        return Ok(WarheadRules::default());
    };
    let parsed = value
        .split(',')
        .map(|part| part.trim().trim_end_matches('%').parse::<u16>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| RulesError::InvalidVerses {
            section: section.to_owned(),
        })?;
    let verses: [u16; 5] = parsed.try_into().map_err(|_| RulesError::InvalidVerses {
        section: section.to_owned(),
    })?;
    Ok(WarheadRules { verses })
}

fn has_key(values: &[(String, String)], key: &str) -> bool {
    values
        .iter()
        .any(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
}

fn text(ini: &Ini, section: &str, key: &str) -> String {
    ini.get(section, key).unwrap_or_default().trim().to_owned()
}

fn list(ini: &Ini, section: &str, key: &str) -> Vec<String> {
    ini.get(section, key)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn number(ini: &Ini, section: &str, key: &'static str) -> Result<i32, RulesError> {
    let Some(value) = ini.get(section, key) else {
        return Ok(0);
    };
    value.parse().map_err(|_| RulesError::InvalidNumber {
        section: section.to_owned(),
        key,
    })
}

fn decimal(ini: &Ini, section: &str, key: &'static str) -> Result<f32, RulesError> {
    let Some(value) = ini.get(section, key) else {
        return Ok(0.0);
    };
    value.parse().map_err(|_| RulesError::InvalidNumber {
        section: section.to_owned(),
        key,
    })
}

fn boolean(ini: &Ini, section: &str, key: &'static str) -> Result<bool, RulesError> {
    boolean_value(ini.get(section, key), section, key)
}

fn boolean_value(
    value: Option<&str>,
    section: &str,
    key: &'static str,
) -> Result<bool, RulesError> {
    let Some(value) = value else {
        return Ok(false);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "1" => Ok(true),
        "no" | "false" | "0" => Ok(false),
        _ => Err(RulesError::InvalidBoolean {
            section: section.to_owned(),
            key,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_rules_typed_sections_and_defaults() {
        let ini = Ini::parse(
            "[VehicleTypes]\n0=TESTUNIT\n[TESTUNIT]\nCost=123\nStrength=300\nPrimary=90mm\nOwner=allies,soviet\nExplodes=yes\n[90mm]\nDamage=30\nROF=50\nRange=4.5\nProjectile=Cannon\nWarhead=AP\n[AP]\nVerses=100%,50%,75%,100%,25%\n",
        );
        let rules = Rules::parse(&ini).unwrap();
        assert!(rules.units.contains_key("TESTUNIT"));
        assert_eq!(rules.units["TESTUNIT"].cost, 123);
        assert_eq!(rules.weapons["90mm"].damage, 30);
        assert_eq!(rules.weapons["90mm"].range, 4.5);
        assert_eq!(rules.warheads["AP"].verses, [100, 50, 75, 100, 25]);
    }

    #[test]
    fn cnc_import_rules_ts_art_overlay() {
        let rules_ini = Ini::parse(
            "[VehicleTypes]\n0=TESTUNIT\n[TESTUNIT]\nImage=RULEIMAGE\nTechLevel=2\nOwner=GDI\nPrerequisite=FACTORY\nPrimary=Laser\nSpeed=6\nStrength=400\nArmor=heavy\nCost=900\nSight=7\n[Laser]\nDamage=25\nROF=30\nRange=5.5\nProjectile=Invisible\nWarhead=LaserWH\nReport=LASER1\n[LaserWH]\nVerses=100%,80%,60%,40%,20%\n",
        );
        let art_ini = Ini::parse("[testunit]\nImage=ARTIMAGE\nFoundation=2x3\n");
        let rules = Rules::parse_ts(&rules_ini, &art_ini).unwrap();
        let unit = &rules.units["TESTUNIT"];
        assert_eq!(unit.image, "ARTIMAGE");
        assert_eq!(unit.foundation, "2x3");
        assert_eq!(unit.owner, ["GDI"]);
        assert_eq!(rules.weapons["Laser"].report, ["LASER1"]);
        assert_eq!(rules.warheads["LaserWH"].verses, [100, 80, 60, 40, 20]);
    }
}
