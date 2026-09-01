//! A semantic score-symbol vocabulary with a canonical SMuFL-name boundary.

use makepad_micro_serde::{DeBin, DeBinErr, SerBin};

/// The visual family of a duration-bearing notehead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NoteheadShape {
    Normal,
    X,
    Diamond,
    TriangleUp,
    TriangleDown,
    CircleX,
    Slash,
}

/// The four outline/fill forms represented by distinct SMuFL notehead glyphs.
/// Quarter notes and all shorter values use `Black`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NoteheadDuration {
    DoubleWhole,
    Whole,
    Half,
    Black,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RestDuration {
    Maxima,
    Longa,
    DoubleWhole,
    Whole,
    Half,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
    SixtyFourth,
    OneTwentyEighth,
    TwoFiftySixth,
    FiveHundredTwelfth,
    OneThousandTwentyFourth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Accidental {
    TripleFlat,
    DoubleFlat,
    Flat,
    Natural,
    Sharp,
    DoubleSharp,
    TripleSharp,
    NaturalFlat,
    NaturalSharp,
    QuarterToneFlat,
    ThreeQuarterTonesFlat,
    QuarterToneSharp,
    ThreeQuarterTonesSharp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SerBin, DeBin)]
pub enum Clef {
    G,
    G8va,
    G8vb,
    G15ma,
    G15mb,
    F,
    F8va,
    F8vb,
    F15ma,
    F15mb,
    C,
    Percussion,
    PercussionAlternate,
    Tab4String,
    Tab6String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlagDuration {
    Eighth,
    Sixteenth,
    ThirtySecond,
    SixtyFourth,
    OneTwentyEighth,
    TwoFiftySixth,
    FiveHundredTwelfth,
    OneThousandTwentyFourth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SerBin, DeBin)]
pub enum Placement {
    Above,
    Below,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SerBin, DeBin)]
pub enum Articulation {
    Accent,
    Staccato,
    Tenuto,
    Staccatissimo,
    Marcato,
    LaissezVibrer,
    Stress,
    SoftAccent,
    AccentStaccato,
    TenutoStaccato,
    MarcatoStaccato,
    MarcatoTenuto,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SerBin, DeBin)]
pub enum DynamicMark {
    Piano,
    Pianissimo,
    Pianississimo,
    Pianissississimo,
    MezzoPiano,
    MezzoForte,
    Forte,
    Fortissimo,
    Fortississimo,
    Fortissississimo,
    FortePiano,
    Sforzando,
    SforzandoPiano,
    Sforzato,
    Rinforzando,
    Niente,
    Mezzo,
    Z,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Digit {
    Zero,
    One,
    Two,
    Three,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SerBin, DeBin)]
pub enum Ornament {
    Trill,
    Turn,
    InvertedTurn,
    TurnWithSlash,
    Mordent,
    ShortTrill,
    Tremblement,
    Schleifer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FermataShape {
    Normal,
    Short,
    Long,
    VeryShort,
    VeryLong,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TremoloStrokes {
    One,
    Two,
    Three,
    Four,
    Five,
}

/// Symbols placed directly by the score engine.
///
/// Conversion at this boundary always uses canonical SMuFL glyph names.
/// `Other` preserves font-independent names outside the working repertoire,
/// including names added by future SMuFL revisions.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Symbol {
    Notehead {
        duration: NoteheadDuration,
        shape: NoteheadShape,
    },
    Rest(RestDuration),
    Accidental(Accidental),
    Clef(Clef),
    Flag {
        duration: FlagDuration,
        direction: Direction,
    },
    Articulation {
        articulation: Articulation,
        placement: Placement,
    },
    Dynamic(DynamicMark),
    TimeSignatureDigit(Digit),
    TimeSignatureCommon,
    TimeSignatureCutCommon,
    TupletDigit(Digit),
    Ornament(Ornament),
    Fermata {
        shape: FermataShape,
        placement: Placement,
    },
    Tremolo(TremoloStrokes),
    AugmentationDot,
    RepeatDot,
    Segno,
    Coda,
    BreathMark,
    Caesura,
    Arpeggio(Direction),
    Other(String),
}

impl Symbol {
    pub fn canonical_name(&self) -> &str {
        match self {
            Self::Notehead { duration, shape } => notehead_name(*duration, *shape),
            Self::Rest(duration) => rest_name(*duration),
            Self::Accidental(accidental) => accidental_name(*accidental),
            Self::Clef(clef) => clef_name(*clef),
            Self::Flag {
                duration,
                direction,
            } => flag_name(*duration, *direction),
            Self::Articulation {
                articulation,
                placement,
            } => articulation_name(*articulation, *placement),
            Self::Dynamic(dynamic) => dynamic_name(*dynamic),
            Self::TimeSignatureDigit(digit) => time_signature_digit_name(*digit),
            Self::TimeSignatureCommon => "timeSigCommon",
            Self::TimeSignatureCutCommon => "timeSigCutCommon",
            Self::TupletDigit(digit) => tuplet_digit_name(*digit),
            Self::Ornament(ornament) => ornament_name(*ornament),
            Self::Fermata { shape, placement } => fermata_name(*shape, *placement),
            Self::Tremolo(strokes) => tremolo_name(*strokes),
            Self::AugmentationDot => "augmentationDot",
            Self::RepeatDot => "repeatDot",
            Self::Segno => "segno",
            Self::Coda => "coda",
            Self::BreathMark => "breathMarkComma",
            Self::Caesura => "caesura",
            Self::Arpeggio(Direction::Up) => "wiggleArpeggiatoUp",
            Self::Arpeggio(Direction::Down) => "wiggleArpeggiatoDown",
            Self::Other(name) => name,
        }
    }

    pub fn from_canonical_name(name: &str) -> Self {
        match name {
            "noteheadDoubleWhole" => notehead(NoteheadDuration::DoubleWhole, NoteheadShape::Normal),
            "noteheadWhole" => notehead(NoteheadDuration::Whole, NoteheadShape::Normal),
            "noteheadHalf" => notehead(NoteheadDuration::Half, NoteheadShape::Normal),
            "noteheadBlack" => notehead(NoteheadDuration::Black, NoteheadShape::Normal),
            "noteheadXDoubleWhole" => notehead(NoteheadDuration::DoubleWhole, NoteheadShape::X),
            "noteheadXWhole" => notehead(NoteheadDuration::Whole, NoteheadShape::X),
            "noteheadXHalf" => notehead(NoteheadDuration::Half, NoteheadShape::X),
            "noteheadXBlack" => notehead(NoteheadDuration::Black, NoteheadShape::X),
            "noteheadDiamondDoubleWhole" => {
                notehead(NoteheadDuration::DoubleWhole, NoteheadShape::Diamond)
            }
            "noteheadDiamondWhole" => notehead(NoteheadDuration::Whole, NoteheadShape::Diamond),
            "noteheadDiamondHalf" => notehead(NoteheadDuration::Half, NoteheadShape::Diamond),
            "noteheadDiamondBlack" => notehead(NoteheadDuration::Black, NoteheadShape::Diamond),
            "noteheadTriangleUpDoubleWhole" => {
                notehead(NoteheadDuration::DoubleWhole, NoteheadShape::TriangleUp)
            }
            "noteheadTriangleUpWhole" => {
                notehead(NoteheadDuration::Whole, NoteheadShape::TriangleUp)
            }
            "noteheadTriangleUpHalf" => notehead(NoteheadDuration::Half, NoteheadShape::TriangleUp),
            "noteheadTriangleUpBlack" => {
                notehead(NoteheadDuration::Black, NoteheadShape::TriangleUp)
            }
            "noteheadTriangleDownDoubleWhole" => {
                notehead(NoteheadDuration::DoubleWhole, NoteheadShape::TriangleDown)
            }
            "noteheadTriangleDownWhole" => {
                notehead(NoteheadDuration::Whole, NoteheadShape::TriangleDown)
            }
            "noteheadTriangleDownHalf" => {
                notehead(NoteheadDuration::Half, NoteheadShape::TriangleDown)
            }
            "noteheadTriangleDownBlack" => {
                notehead(NoteheadDuration::Black, NoteheadShape::TriangleDown)
            }
            "noteheadCircleXDoubleWhole" => {
                notehead(NoteheadDuration::DoubleWhole, NoteheadShape::CircleX)
            }
            "noteheadCircleXWhole" => notehead(NoteheadDuration::Whole, NoteheadShape::CircleX),
            "noteheadCircleXHalf" => notehead(NoteheadDuration::Half, NoteheadShape::CircleX),
            "noteheadCircleX" => notehead(NoteheadDuration::Black, NoteheadShape::CircleX),
            "noteheadSlashWhiteDoubleWhole" => {
                notehead(NoteheadDuration::DoubleWhole, NoteheadShape::Slash)
            }
            "noteheadSlashWhiteWhole" => notehead(NoteheadDuration::Whole, NoteheadShape::Slash),
            "noteheadSlashWhiteHalf" => notehead(NoteheadDuration::Half, NoteheadShape::Slash),
            "noteheadSlashHorizontalEnds" => {
                notehead(NoteheadDuration::Black, NoteheadShape::Slash)
            }
            "restMaxima" => Self::Rest(RestDuration::Maxima),
            "restLonga" => Self::Rest(RestDuration::Longa),
            "restDoubleWhole" => Self::Rest(RestDuration::DoubleWhole),
            "restWhole" => Self::Rest(RestDuration::Whole),
            "restHalf" => Self::Rest(RestDuration::Half),
            "restQuarter" => Self::Rest(RestDuration::Quarter),
            "rest8th" => Self::Rest(RestDuration::Eighth),
            "rest16th" => Self::Rest(RestDuration::Sixteenth),
            "rest32nd" => Self::Rest(RestDuration::ThirtySecond),
            "rest64th" => Self::Rest(RestDuration::SixtyFourth),
            "rest128th" => Self::Rest(RestDuration::OneTwentyEighth),
            "rest256th" => Self::Rest(RestDuration::TwoFiftySixth),
            "rest512th" => Self::Rest(RestDuration::FiveHundredTwelfth),
            "rest1024th" => Self::Rest(RestDuration::OneThousandTwentyFourth),
            "accidentalTripleFlat" => Self::Accidental(Accidental::TripleFlat),
            "accidentalDoubleFlat" => Self::Accidental(Accidental::DoubleFlat),
            "accidentalFlat" => Self::Accidental(Accidental::Flat),
            "accidentalNatural" => Self::Accidental(Accidental::Natural),
            "accidentalSharp" => Self::Accidental(Accidental::Sharp),
            "accidentalDoubleSharp" => Self::Accidental(Accidental::DoubleSharp),
            "accidentalTripleSharp" => Self::Accidental(Accidental::TripleSharp),
            "accidentalNaturalFlat" => Self::Accidental(Accidental::NaturalFlat),
            "accidentalNaturalSharp" => Self::Accidental(Accidental::NaturalSharp),
            "accidentalQuarterToneFlatStein" => Self::Accidental(Accidental::QuarterToneFlat),
            "accidentalThreeQuarterTonesFlatZimmermann" => {
                Self::Accidental(Accidental::ThreeQuarterTonesFlat)
            }
            "accidentalQuarterToneSharpStein" => Self::Accidental(Accidental::QuarterToneSharp),
            "accidentalThreeQuarterTonesSharpStein" => {
                Self::Accidental(Accidental::ThreeQuarterTonesSharp)
            }
            "gClef" => Self::Clef(Clef::G),
            "gClef8va" => Self::Clef(Clef::G8va),
            "gClef8vb" => Self::Clef(Clef::G8vb),
            "gClef15ma" => Self::Clef(Clef::G15ma),
            "gClef15mb" => Self::Clef(Clef::G15mb),
            "fClef" => Self::Clef(Clef::F),
            "fClef8va" => Self::Clef(Clef::F8va),
            "fClef8vb" => Self::Clef(Clef::F8vb),
            "fClef15ma" => Self::Clef(Clef::F15ma),
            "fClef15mb" => Self::Clef(Clef::F15mb),
            "cClef" => Self::Clef(Clef::C),
            "unpitchedPercussionClef1" => Self::Clef(Clef::Percussion),
            "unpitchedPercussionClef2" => Self::Clef(Clef::PercussionAlternate),
            "4stringTabClef" => Self::Clef(Clef::Tab4String),
            "6stringTabClef" => Self::Clef(Clef::Tab6String),
            "timeSigCommon" => Self::TimeSignatureCommon,
            "timeSigCutCommon" => Self::TimeSignatureCutCommon,
            "ornamentTrill" => Self::Ornament(Ornament::Trill),
            "ornamentTurn" => Self::Ornament(Ornament::Turn),
            "ornamentTurnInverted" => Self::Ornament(Ornament::InvertedTurn),
            "ornamentTurnSlash" => Self::Ornament(Ornament::TurnWithSlash),
            "ornamentMordent" => Self::Ornament(Ornament::Mordent),
            "ornamentShortTrill" => Self::Ornament(Ornament::ShortTrill),
            "ornamentTremblement" => Self::Ornament(Ornament::Tremblement),
            "ornamentSchleifer" => Self::Ornament(Ornament::Schleifer),
            "augmentationDot" => Self::AugmentationDot,
            "repeatDot" => Self::RepeatDot,
            "segno" => Self::Segno,
            "coda" => Self::Coda,
            "breathMarkComma" => Self::BreathMark,
            "caesura" => Self::Caesura,
            "wiggleArpeggiatoUp" => Self::Arpeggio(Direction::Up),
            "wiggleArpeggiatoDown" => Self::Arpeggio(Direction::Down),
            _ => from_patterned_name(name).unwrap_or_else(|| Self::Other(name.to_string())),
        }
    }
}

impl From<&str> for Symbol {
    fn from(value: &str) -> Self {
        Self::from_canonical_name(value)
    }
}

fn notehead(duration: NoteheadDuration, shape: NoteheadShape) -> Symbol {
    Symbol::Notehead { duration, shape }
}

fn notehead_name(duration: NoteheadDuration, shape: NoteheadShape) -> &'static str {
    match (duration, shape) {
        (NoteheadDuration::DoubleWhole, NoteheadShape::Normal) => "noteheadDoubleWhole",
        (NoteheadDuration::Whole, NoteheadShape::Normal) => "noteheadWhole",
        (NoteheadDuration::Half, NoteheadShape::Normal) => "noteheadHalf",
        (NoteheadDuration::Black, NoteheadShape::Normal) => "noteheadBlack",
        (NoteheadDuration::DoubleWhole, NoteheadShape::X) => "noteheadXDoubleWhole",
        (NoteheadDuration::Whole, NoteheadShape::X) => "noteheadXWhole",
        (NoteheadDuration::Half, NoteheadShape::X) => "noteheadXHalf",
        (NoteheadDuration::Black, NoteheadShape::X) => "noteheadXBlack",
        (NoteheadDuration::DoubleWhole, NoteheadShape::Diamond) => "noteheadDiamondDoubleWhole",
        (NoteheadDuration::Whole, NoteheadShape::Diamond) => "noteheadDiamondWhole",
        (NoteheadDuration::Half, NoteheadShape::Diamond) => "noteheadDiamondHalf",
        (NoteheadDuration::Black, NoteheadShape::Diamond) => "noteheadDiamondBlack",
        (NoteheadDuration::DoubleWhole, NoteheadShape::TriangleUp) => {
            "noteheadTriangleUpDoubleWhole"
        }
        (NoteheadDuration::Whole, NoteheadShape::TriangleUp) => "noteheadTriangleUpWhole",
        (NoteheadDuration::Half, NoteheadShape::TriangleUp) => "noteheadTriangleUpHalf",
        (NoteheadDuration::Black, NoteheadShape::TriangleUp) => "noteheadTriangleUpBlack",
        (NoteheadDuration::DoubleWhole, NoteheadShape::TriangleDown) => {
            "noteheadTriangleDownDoubleWhole"
        }
        (NoteheadDuration::Whole, NoteheadShape::TriangleDown) => "noteheadTriangleDownWhole",
        (NoteheadDuration::Half, NoteheadShape::TriangleDown) => "noteheadTriangleDownHalf",
        (NoteheadDuration::Black, NoteheadShape::TriangleDown) => "noteheadTriangleDownBlack",
        (NoteheadDuration::DoubleWhole, NoteheadShape::CircleX) => "noteheadCircleXDoubleWhole",
        (NoteheadDuration::Whole, NoteheadShape::CircleX) => "noteheadCircleXWhole",
        (NoteheadDuration::Half, NoteheadShape::CircleX) => "noteheadCircleXHalf",
        (NoteheadDuration::Black, NoteheadShape::CircleX) => "noteheadCircleX",
        (NoteheadDuration::DoubleWhole, NoteheadShape::Slash) => "noteheadSlashWhiteDoubleWhole",
        (NoteheadDuration::Whole, NoteheadShape::Slash) => "noteheadSlashWhiteWhole",
        (NoteheadDuration::Half, NoteheadShape::Slash) => "noteheadSlashWhiteHalf",
        (NoteheadDuration::Black, NoteheadShape::Slash) => "noteheadSlashHorizontalEnds",
    }
}

fn rest_name(duration: RestDuration) -> &'static str {
    match duration {
        RestDuration::Maxima => "restMaxima",
        RestDuration::Longa => "restLonga",
        RestDuration::DoubleWhole => "restDoubleWhole",
        RestDuration::Whole => "restWhole",
        RestDuration::Half => "restHalf",
        RestDuration::Quarter => "restQuarter",
        RestDuration::Eighth => "rest8th",
        RestDuration::Sixteenth => "rest16th",
        RestDuration::ThirtySecond => "rest32nd",
        RestDuration::SixtyFourth => "rest64th",
        RestDuration::OneTwentyEighth => "rest128th",
        RestDuration::TwoFiftySixth => "rest256th",
        RestDuration::FiveHundredTwelfth => "rest512th",
        RestDuration::OneThousandTwentyFourth => "rest1024th",
    }
}

fn accidental_name(accidental: Accidental) -> &'static str {
    match accidental {
        Accidental::TripleFlat => "accidentalTripleFlat",
        Accidental::DoubleFlat => "accidentalDoubleFlat",
        Accidental::Flat => "accidentalFlat",
        Accidental::Natural => "accidentalNatural",
        Accidental::Sharp => "accidentalSharp",
        Accidental::DoubleSharp => "accidentalDoubleSharp",
        Accidental::TripleSharp => "accidentalTripleSharp",
        Accidental::NaturalFlat => "accidentalNaturalFlat",
        Accidental::NaturalSharp => "accidentalNaturalSharp",
        Accidental::QuarterToneFlat => "accidentalQuarterToneFlatStein",
        Accidental::ThreeQuarterTonesFlat => "accidentalThreeQuarterTonesFlatZimmermann",
        Accidental::QuarterToneSharp => "accidentalQuarterToneSharpStein",
        Accidental::ThreeQuarterTonesSharp => "accidentalThreeQuarterTonesSharpStein",
    }
}

fn clef_name(clef: Clef) -> &'static str {
    match clef {
        Clef::G => "gClef",
        Clef::G8va => "gClef8va",
        Clef::G8vb => "gClef8vb",
        Clef::G15ma => "gClef15ma",
        Clef::G15mb => "gClef15mb",
        Clef::F => "fClef",
        Clef::F8va => "fClef8va",
        Clef::F8vb => "fClef8vb",
        Clef::F15ma => "fClef15ma",
        Clef::F15mb => "fClef15mb",
        Clef::C => "cClef",
        Clef::Percussion => "unpitchedPercussionClef1",
        Clef::PercussionAlternate => "unpitchedPercussionClef2",
        Clef::Tab4String => "4stringTabClef",
        Clef::Tab6String => "6stringTabClef",
    }
}

fn flag_name(duration: FlagDuration, direction: Direction) -> &'static str {
    match (duration, direction) {
        (FlagDuration::Eighth, Direction::Up) => "flag8thUp",
        (FlagDuration::Eighth, Direction::Down) => "flag8thDown",
        (FlagDuration::Sixteenth, Direction::Up) => "flag16thUp",
        (FlagDuration::Sixteenth, Direction::Down) => "flag16thDown",
        (FlagDuration::ThirtySecond, Direction::Up) => "flag32ndUp",
        (FlagDuration::ThirtySecond, Direction::Down) => "flag32ndDown",
        (FlagDuration::SixtyFourth, Direction::Up) => "flag64thUp",
        (FlagDuration::SixtyFourth, Direction::Down) => "flag64thDown",
        (FlagDuration::OneTwentyEighth, Direction::Up) => "flag128thUp",
        (FlagDuration::OneTwentyEighth, Direction::Down) => "flag128thDown",
        (FlagDuration::TwoFiftySixth, Direction::Up) => "flag256thUp",
        (FlagDuration::TwoFiftySixth, Direction::Down) => "flag256thDown",
        (FlagDuration::FiveHundredTwelfth, Direction::Up) => "flag512thUp",
        (FlagDuration::FiveHundredTwelfth, Direction::Down) => "flag512thDown",
        (FlagDuration::OneThousandTwentyFourth, Direction::Up) => "flag1024thUp",
        (FlagDuration::OneThousandTwentyFourth, Direction::Down) => "flag1024thDown",
    }
}

fn articulation_name(articulation: Articulation, placement: Placement) -> &'static str {
    match (articulation, placement) {
        (Articulation::Accent, Placement::Above) => "articAccentAbove",
        (Articulation::Accent, Placement::Below) => "articAccentBelow",
        (Articulation::Staccato, Placement::Above) => "articStaccatoAbove",
        (Articulation::Staccato, Placement::Below) => "articStaccatoBelow",
        (Articulation::Tenuto, Placement::Above) => "articTenutoAbove",
        (Articulation::Tenuto, Placement::Below) => "articTenutoBelow",
        (Articulation::Staccatissimo, Placement::Above) => "articStaccatissimoAbove",
        (Articulation::Staccatissimo, Placement::Below) => "articStaccatissimoBelow",
        (Articulation::Marcato, Placement::Above) => "articMarcatoAbove",
        (Articulation::Marcato, Placement::Below) => "articMarcatoBelow",
        (Articulation::LaissezVibrer, Placement::Above) => "articLaissezVibrerAbove",
        (Articulation::LaissezVibrer, Placement::Below) => "articLaissezVibrerBelow",
        (Articulation::Stress, Placement::Above) => "articStressAbove",
        (Articulation::Stress, Placement::Below) => "articStressBelow",
        (Articulation::SoftAccent, Placement::Above) => "articSoftAccentAbove",
        (Articulation::SoftAccent, Placement::Below) => "articSoftAccentBelow",
        (Articulation::AccentStaccato, Placement::Above) => "articAccentStaccatoAbove",
        (Articulation::AccentStaccato, Placement::Below) => "articAccentStaccatoBelow",
        (Articulation::TenutoStaccato, Placement::Above) => "articTenutoStaccatoAbove",
        (Articulation::TenutoStaccato, Placement::Below) => "articTenutoStaccatoBelow",
        (Articulation::MarcatoStaccato, Placement::Above) => "articMarcatoStaccatoAbove",
        (Articulation::MarcatoStaccato, Placement::Below) => "articMarcatoStaccatoBelow",
        (Articulation::MarcatoTenuto, Placement::Above) => "articMarcatoTenutoAbove",
        (Articulation::MarcatoTenuto, Placement::Below) => "articMarcatoTenutoBelow",
    }
}

fn dynamic_name(dynamic: DynamicMark) -> &'static str {
    match dynamic {
        DynamicMark::Piano => "dynamicPiano",
        DynamicMark::Pianissimo => "dynamicPP",
        DynamicMark::Pianississimo => "dynamicPPP",
        DynamicMark::Pianissississimo => "dynamicPPPP",
        DynamicMark::MezzoPiano => "dynamicMP",
        DynamicMark::MezzoForte => "dynamicMF",
        DynamicMark::Forte => "dynamicForte",
        DynamicMark::Fortissimo => "dynamicFF",
        DynamicMark::Fortississimo => "dynamicFFF",
        DynamicMark::Fortissississimo => "dynamicFFFF",
        DynamicMark::FortePiano => "dynamicFortePiano",
        DynamicMark::Sforzando => "dynamicSforzando",
        DynamicMark::SforzandoPiano => "dynamicSforzandoPiano",
        DynamicMark::Sforzato => "dynamicSforzato",
        DynamicMark::Rinforzando => "dynamicRinforzando",
        DynamicMark::Niente => "dynamicNiente",
        DynamicMark::Mezzo => "dynamicMezzo",
        DynamicMark::Z => "dynamicZ",
    }
}

fn time_signature_digit_name(digit: Digit) -> &'static str {
    match digit {
        Digit::Zero => "timeSig0",
        Digit::One => "timeSig1",
        Digit::Two => "timeSig2",
        Digit::Three => "timeSig3",
        Digit::Four => "timeSig4",
        Digit::Five => "timeSig5",
        Digit::Six => "timeSig6",
        Digit::Seven => "timeSig7",
        Digit::Eight => "timeSig8",
        Digit::Nine => "timeSig9",
    }
}

fn tuplet_digit_name(digit: Digit) -> &'static str {
    match digit {
        Digit::Zero => "tuplet0",
        Digit::One => "tuplet1",
        Digit::Two => "tuplet2",
        Digit::Three => "tuplet3",
        Digit::Four => "tuplet4",
        Digit::Five => "tuplet5",
        Digit::Six => "tuplet6",
        Digit::Seven => "tuplet7",
        Digit::Eight => "tuplet8",
        Digit::Nine => "tuplet9",
    }
}

fn ornament_name(ornament: Ornament) -> &'static str {
    match ornament {
        Ornament::Trill => "ornamentTrill",
        Ornament::Turn => "ornamentTurn",
        Ornament::InvertedTurn => "ornamentTurnInverted",
        Ornament::TurnWithSlash => "ornamentTurnSlash",
        Ornament::Mordent => "ornamentMordent",
        Ornament::ShortTrill => "ornamentShortTrill",
        Ornament::Tremblement => "ornamentTremblement",
        Ornament::Schleifer => "ornamentSchleifer",
    }
}

fn fermata_name(shape: FermataShape, placement: Placement) -> &'static str {
    match (shape, placement) {
        (FermataShape::Normal, Placement::Above) => "fermataAbove",
        (FermataShape::Normal, Placement::Below) => "fermataBelow",
        (FermataShape::Short, Placement::Above) => "fermataShortAbove",
        (FermataShape::Short, Placement::Below) => "fermataShortBelow",
        (FermataShape::Long, Placement::Above) => "fermataLongAbove",
        (FermataShape::Long, Placement::Below) => "fermataLongBelow",
        (FermataShape::VeryShort, Placement::Above) => "fermataVeryShortAbove",
        (FermataShape::VeryShort, Placement::Below) => "fermataVeryShortBelow",
        (FermataShape::VeryLong, Placement::Above) => "fermataVeryLongAbove",
        (FermataShape::VeryLong, Placement::Below) => "fermataVeryLongBelow",
    }
}

fn tremolo_name(strokes: TremoloStrokes) -> &'static str {
    match strokes {
        TremoloStrokes::One => "tremolo1",
        TremoloStrokes::Two => "tremolo2",
        TremoloStrokes::Three => "tremolo3",
        TremoloStrokes::Four => "tremolo4",
        TremoloStrokes::Five => "tremolo5",
    }
}

fn from_patterned_name(name: &str) -> Option<Symbol> {
    let digit = |digit| match digit {
        '0' => Some(Digit::Zero),
        '1' => Some(Digit::One),
        '2' => Some(Digit::Two),
        '3' => Some(Digit::Three),
        '4' => Some(Digit::Four),
        '5' => Some(Digit::Five),
        '6' => Some(Digit::Six),
        '7' => Some(Digit::Seven),
        '8' => Some(Digit::Eight),
        '9' => Some(Digit::Nine),
        _ => None,
    };
    if let Some(suffix) = name.strip_prefix("timeSig") {
        if suffix.len() == 1 {
            return digit(suffix.chars().next()?).map(Symbol::TimeSignatureDigit);
        }
    }
    if let Some(suffix) = name.strip_prefix("tuplet") {
        if suffix.len() == 1 {
            return digit(suffix.chars().next()?).map(Symbol::TupletDigit);
        }
    }

    Some(match name {
        "flag8thUp" => Symbol::Flag {
            duration: FlagDuration::Eighth,
            direction: Direction::Up,
        },
        "flag8thDown" => Symbol::Flag {
            duration: FlagDuration::Eighth,
            direction: Direction::Down,
        },
        "flag16thUp" => flag(FlagDuration::Sixteenth, Direction::Up),
        "flag16thDown" => flag(FlagDuration::Sixteenth, Direction::Down),
        "flag32ndUp" => flag(FlagDuration::ThirtySecond, Direction::Up),
        "flag32ndDown" => flag(FlagDuration::ThirtySecond, Direction::Down),
        "flag64thUp" => flag(FlagDuration::SixtyFourth, Direction::Up),
        "flag64thDown" => flag(FlagDuration::SixtyFourth, Direction::Down),
        "flag128thUp" => flag(FlagDuration::OneTwentyEighth, Direction::Up),
        "flag128thDown" => flag(FlagDuration::OneTwentyEighth, Direction::Down),
        "flag256thUp" => flag(FlagDuration::TwoFiftySixth, Direction::Up),
        "flag256thDown" => flag(FlagDuration::TwoFiftySixth, Direction::Down),
        "flag512thUp" => flag(FlagDuration::FiveHundredTwelfth, Direction::Up),
        "flag512thDown" => flag(FlagDuration::FiveHundredTwelfth, Direction::Down),
        "flag1024thUp" => flag(FlagDuration::OneThousandTwentyFourth, Direction::Up),
        "flag1024thDown" => flag(FlagDuration::OneThousandTwentyFourth, Direction::Down),
        "dynamicPiano" => Symbol::Dynamic(DynamicMark::Piano),
        "dynamicPP" => Symbol::Dynamic(DynamicMark::Pianissimo),
        "dynamicPPP" => Symbol::Dynamic(DynamicMark::Pianississimo),
        "dynamicPPPP" => Symbol::Dynamic(DynamicMark::Pianissississimo),
        "dynamicMP" => Symbol::Dynamic(DynamicMark::MezzoPiano),
        "dynamicMF" => Symbol::Dynamic(DynamicMark::MezzoForte),
        "dynamicForte" => Symbol::Dynamic(DynamicMark::Forte),
        "dynamicFF" => Symbol::Dynamic(DynamicMark::Fortissimo),
        "dynamicFFF" => Symbol::Dynamic(DynamicMark::Fortississimo),
        "dynamicFFFF" => Symbol::Dynamic(DynamicMark::Fortissississimo),
        "dynamicFortePiano" => Symbol::Dynamic(DynamicMark::FortePiano),
        "dynamicSforzando" => Symbol::Dynamic(DynamicMark::Sforzando),
        "dynamicSforzandoPiano" => Symbol::Dynamic(DynamicMark::SforzandoPiano),
        "dynamicSforzato" => Symbol::Dynamic(DynamicMark::Sforzato),
        "dynamicRinforzando" => Symbol::Dynamic(DynamicMark::Rinforzando),
        "dynamicNiente" => Symbol::Dynamic(DynamicMark::Niente),
        "dynamicMezzo" => Symbol::Dynamic(DynamicMark::Mezzo),
        "dynamicZ" => Symbol::Dynamic(DynamicMark::Z),
        "tremolo1" => Symbol::Tremolo(TremoloStrokes::One),
        "tremolo2" => Symbol::Tremolo(TremoloStrokes::Two),
        "tremolo3" => Symbol::Tremolo(TremoloStrokes::Three),
        "tremolo4" => Symbol::Tremolo(TremoloStrokes::Four),
        "tremolo5" => Symbol::Tremolo(TremoloStrokes::Five),
        _ => {
            if let Some(symbol) = articulation_from_name(name) {
                symbol
            } else if let Some(symbol) = fermata_from_name(name) {
                symbol
            } else {
                return None;
            }
        }
    })
}

fn flag(duration: FlagDuration, direction: Direction) -> Symbol {
    Symbol::Flag {
        duration,
        direction,
    }
}

fn articulation_from_name(name: &str) -> Option<Symbol> {
    let (base, placement) = name
        .strip_suffix("Above")
        .map(|base| (base, Placement::Above))
        .or_else(|| name.strip_suffix("Below").map(|base| (base, Placement::Below)))?;
    let articulation = match base {
        "articAccent" => Articulation::Accent,
        "articStaccato" => Articulation::Staccato,
        "articTenuto" => Articulation::Tenuto,
        "articStaccatissimo" => Articulation::Staccatissimo,
        "articMarcato" => Articulation::Marcato,
        "articLaissezVibrer" => Articulation::LaissezVibrer,
        "articStress" => Articulation::Stress,
        "articSoftAccent" => Articulation::SoftAccent,
        "articAccentStaccato" => Articulation::AccentStaccato,
        "articTenutoStaccato" => Articulation::TenutoStaccato,
        "articMarcatoStaccato" => Articulation::MarcatoStaccato,
        "articMarcatoTenuto" => Articulation::MarcatoTenuto,
        _ => return None,
    };
    Some(Symbol::Articulation {
        articulation,
        placement,
    })
}

fn fermata_from_name(name: &str) -> Option<Symbol> {
    let (base, placement) = name
        .strip_suffix("Above")
        .map(|base| (base, Placement::Above))
        .or_else(|| name.strip_suffix("Below").map(|base| (base, Placement::Below)))?;
    let shape = match base {
        "fermata" => FermataShape::Normal,
        "fermataShort" => FermataShape::Short,
        "fermataLong" => FermataShape::Long,
        "fermataVeryShort" => FermataShape::VeryShort,
        "fermataVeryLong" => FermataShape::VeryLong,
        _ => return None,
    };
    Some(Symbol::Fermata { shape, placement })
}
