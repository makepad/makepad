pub enum Kind {
    One = 1,
    #[cfg(feature = "wide")]
    Wide = 20,
}

include!(concat!(env!("OUT_DIR"), "/total.rs"));
