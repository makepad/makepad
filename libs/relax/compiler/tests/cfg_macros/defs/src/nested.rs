mod inner {
    #[cfg(feature = "tweaker")]
    #[cfg_attr(all(), macro_export)]
    macro_rules! nested {
        () => { 71 };
    }
    #[cfg(not(feature = "tweaker"))]
    #[macro_export]
    macro_rules! nested {
        () => { 72 };
    }
    #[cfg(any())]
    mod inactive {
        #[macro_export]
        macro_rules! nested {
            () => { 99 };
        }
    }
}
