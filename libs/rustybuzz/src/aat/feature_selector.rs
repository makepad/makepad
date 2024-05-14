// FeatureType::Ligatures
pub const COMMON_LIGATURES_ON: u8 = 2;
pub const COMMON_LIGATURES_OFF: u8 = 3;
pub const RARE_LIGATURES_ON: u8 = 4;
pub const RARE_LIGATURES_OFF: u8 = 5;
pub const CONTEXTUAL_LIGATURES_ON: u8 = 18;
pub const CONTEXTUAL_LIGATURES_OFF: u8 = 19;
pub const HISTORICAL_LIGATURES_ON: u8 = 20;
pub const HISTORICAL_LIGATURES_OFF: u8 = 21;

// FeatureType::LetterCase
pub const SMALL_CAPS: u8 = 3; // deprecated

// FeatureType::VerticalSubstitution
pub const SUBSTITUTE_VERTICAL_FORMS_ON: u8 = 0;
pub const SUBSTITUTE_VERTICAL_FORMS_OFF: u8 = 1;

// FeatureType::NumberSpacing
pub const MONOSPACED_NUMBERS: u8 = 0;
pub const PROPORTIONAL_NUMBERS: u8 = 1;

// FeatureType::VerticalPosition
pub const NORMAL_POSITION: u8 = 0;
pub const SUPERIORS: u8 = 1;
pub const INFERIORS: u8 = 2;
pub const ORDINALS: u8 = 3;
pub const SCIENTIFIC_INFERIORS: u8 = 4;

// FeatureType::Fractions
pub const NO_FRACTIONS: u8 = 0;
pub const VERTICAL_FRACTIONS: u8 = 1;
pub const DIAGONAL_FRACTIONS: u8 = 2;

// FeatureType::TypographicExtras
pub const SLASHED_ZERO_ON: u8 = 4;
pub const SLASHED_ZERO_OFF: u8 = 5;

// FeatureType::MathematicalExtras
pub const MATHEMATICAL_GREEK_ON: u8 = 10;
pub const MATHEMATICAL_GREEK_OFF: u8 = 11;

// FeatureType::StyleOptions
pub const NO_STYLE_OPTIONS: u8 = 0;
pub const TITLING_CAPS: u8 = 4;

// FeatureType::CharacterShape
pub const TRADITIONAL_CHARACTERS: u8 = 0;
pub const SIMPLIFIED_CHARACTERS: u8 = 1;
pub const JIS1978_CHARACTERS: u8 = 2;
pub const JIS1983_CHARACTERS: u8 = 3;
pub const JIS1990_CHARACTERS: u8 = 4;
pub const EXPERT_CHARACTERS: u8 = 10;
pub const JIS2004_CHARACTERS: u8 = 11;
pub const HOJO_CHARACTERS: u8 = 12;
pub const NLCCHARACTERS: u8 = 13;
pub const TRADITIONAL_NAMES_CHARACTERS: u8 = 14;

// FeatureType::NumberCase
pub const LOWER_CASE_NUMBERS: u8 = 0;
pub const UPPER_CASE_NUMBERS: u8 = 1;

// FeatureType::TextSpacing
pub const PROPORTIONAL_TEXT: u8 = 0;
pub const MONOSPACED_TEXT: u8 = 1;
pub const HALF_WIDTH_TEXT: u8 = 2;
pub const THIRD_WIDTH_TEXT: u8 = 3;
pub const QUARTER_WIDTH_TEXT: u8 = 4;
pub const ALT_PROPORTIONAL_TEXT: u8 = 5;
pub const ALT_HALF_WIDTH_TEXT: u8 = 6;

// FeatureType::Transliteration
pub const NO_TRANSLITERATION: u8 = 0;
pub const HANJA_TO_HANGUL: u8 = 1;

// FeatureType::RubyKana
pub const RUBY_KANA_ON: u8 = 2;
pub const RUBY_KANA_OFF: u8 = 3;

// FeatureType::ItalicCjkRoman
pub const CJK_ITALIC_ROMAN_ON: u8 = 2;
pub const CJK_ITALIC_ROMAN_OFF: u8 = 3;

// FeatureType::CaseSensitiveLayout
pub const CASE_SENSITIVE_LAYOUT_ON: u8 = 0;
pub const CASE_SENSITIVE_LAYOUT_OFF: u8 = 1;
pub const CASE_SENSITIVE_SPACING_ON: u8 = 2;
pub const CASE_SENSITIVE_SPACING_OFF: u8 = 3;

// FeatureType::AlternateKana
pub const ALTERNATE_HORIZ_KANA_ON: u8 = 0;
pub const ALTERNATE_HORIZ_KANA_OFF: u8 = 1;
pub const ALTERNATE_VERT_KANA_ON: u8 = 2;
pub const ALTERNATE_VERT_KANA_OFF: u8 = 3;

// FeatureType::StylisticAlternatives
pub const STYLISTIC_ALT_ONE_ON: u8 = 2;
pub const STYLISTIC_ALT_ONE_OFF: u8 = 3;
pub const STYLISTIC_ALT_TWO_ON: u8 = 4;
pub const STYLISTIC_ALT_TWO_OFF: u8 = 5;
pub const STYLISTIC_ALT_THREE_ON: u8 = 6;
pub const STYLISTIC_ALT_THREE_OFF: u8 = 7;
pub const STYLISTIC_ALT_FOUR_ON: u8 = 8;
pub const STYLISTIC_ALT_FOUR_OFF: u8 = 9;
pub const STYLISTIC_ALT_FIVE_ON: u8 = 10;
pub const STYLISTIC_ALT_FIVE_OFF: u8 = 11;
pub const STYLISTIC_ALT_SIX_ON: u8 = 12;
pub const STYLISTIC_ALT_SIX_OFF: u8 = 13;
pub const STYLISTIC_ALT_SEVEN_ON: u8 = 14;
pub const STYLISTIC_ALT_SEVEN_OFF: u8 = 15;
pub const STYLISTIC_ALT_EIGHT_ON: u8 = 16;
pub const STYLISTIC_ALT_EIGHT_OFF: u8 = 17;
pub const STYLISTIC_ALT_NINE_ON: u8 = 18;
pub const STYLISTIC_ALT_NINE_OFF: u8 = 19;
pub const STYLISTIC_ALT_TEN_ON: u8 = 20;
pub const STYLISTIC_ALT_TEN_OFF: u8 = 21;
pub const STYLISTIC_ALT_ELEVEN_ON: u8 = 22;
pub const STYLISTIC_ALT_ELEVEN_OFF: u8 = 23;
pub const STYLISTIC_ALT_TWELVE_ON: u8 = 24;
pub const STYLISTIC_ALT_TWELVE_OFF: u8 = 25;
pub const STYLISTIC_ALT_THIRTEEN_ON: u8 = 26;
pub const STYLISTIC_ALT_THIRTEEN_OFF: u8 = 27;
pub const STYLISTIC_ALT_FOURTEEN_ON: u8 = 28;
pub const STYLISTIC_ALT_FOURTEEN_OFF: u8 = 29;
pub const STYLISTIC_ALT_FIFTEEN_ON: u8 = 30;
pub const STYLISTIC_ALT_FIFTEEN_OFF: u8 = 31;
pub const STYLISTIC_ALT_SIXTEEN_ON: u8 = 32;
pub const STYLISTIC_ALT_SIXTEEN_OFF: u8 = 33;
pub const STYLISTIC_ALT_SEVENTEEN_ON: u8 = 34;
pub const STYLISTIC_ALT_SEVENTEEN_OFF: u8 = 35;
pub const STYLISTIC_ALT_EIGHTEEN_ON: u8 = 36;
pub const STYLISTIC_ALT_EIGHTEEN_OFF: u8 = 37;
pub const STYLISTIC_ALT_NINETEEN_ON: u8 = 38;
pub const STYLISTIC_ALT_NINETEEN_OFF: u8 = 39;
pub const STYLISTIC_ALT_TWENTY_ON: u8 = 40;
pub const STYLISTIC_ALT_TWENTY_OFF: u8 = 41;

// FeatureType::ContextualAlternatives
pub const CONTEXTUAL_ALTERNATES_ON: u8 = 0;
pub const CONTEXTUAL_ALTERNATES_OFF: u8 = 1;
pub const SWASH_ALTERNATES_ON: u8 = 2;
pub const SWASH_ALTERNATES_OFF: u8 = 3;
pub const CONTEXTUAL_SWASH_ALTERNATES_ON: u8 = 4;
pub const CONTEXTUAL_SWASH_ALTERNATES_OFF: u8 = 5;

// FeatureType::LowerCase
pub const DEFAULT_LOWER_CASE: u8 = 0;
pub const LOWER_CASE_SMALL_CAPS: u8 = 1;
pub const LOWER_CASE_PETITE_CAPS: u8 = 2;

// FeatureType::UpperCase
pub const DEFAULT_UPPER_CASE: u8 = 0;
pub const UPPER_CASE_SMALL_CAPS: u8 = 1;
pub const UPPER_CASE_PETITE_CAPS: u8 = 2;
