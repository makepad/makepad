pub struct Ratio(pub u64, pub u64);

#[cfg(feature = "df-wide")]
pub type WideRatio = (df_wide::Wide, df_wide::Wide);

#[cfg(feature = "df-wide")]
pub fn wide_ratio(n: u64, d: u64) -> WideRatio {
    (df_wide::Wide(n), df_wide::Wide(d))
}
