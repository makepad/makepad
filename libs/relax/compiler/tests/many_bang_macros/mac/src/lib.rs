//! 64 function-like proc macros: `value_N!()` expands to `N` (a graph with more than the 56
//! function-like proc macros the u8 hosted-macro ids could name; Robrix has more).
use proc_macro::TokenStream;

macro_rules! values {
    ($($name:ident = $n:literal),* $(,)?) => { $(
        #[proc_macro]
        pub fn $name(_input: TokenStream) -> TokenStream {
            concat!(stringify!($n), "u32").parse().unwrap()
        }
    )* };
}

values!(
    value_0 = 0, value_1 = 1, value_2 = 2, value_3 = 3, value_4 = 4, value_5 = 5, value_6 = 6, value_7 = 7,
    value_8 = 8, value_9 = 9, value_10 = 10, value_11 = 11, value_12 = 12, value_13 = 13, value_14 = 14, value_15 = 15,
    value_16 = 16, value_17 = 17, value_18 = 18, value_19 = 19, value_20 = 20, value_21 = 21, value_22 = 22, value_23 = 23,
    value_24 = 24, value_25 = 25, value_26 = 26, value_27 = 27, value_28 = 28, value_29 = 29, value_30 = 30, value_31 = 31,
    value_32 = 32, value_33 = 33, value_34 = 34, value_35 = 35, value_36 = 36, value_37 = 37, value_38 = 38, value_39 = 39,
    value_40 = 40, value_41 = 41, value_42 = 42, value_43 = 43, value_44 = 44, value_45 = 45, value_46 = 46, value_47 = 47,
    value_48 = 48, value_49 = 49, value_50 = 50, value_51 = 51, value_52 = 52, value_53 = 53, value_54 = 54, value_55 = 55,
    value_56 = 56, value_57 = 57, value_58 = 58, value_59 = 59, value_60 = 60, value_61 = 61, value_62 = 62, value_63 = 63,
);
