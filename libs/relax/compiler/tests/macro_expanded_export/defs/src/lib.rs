// Resolve a written export before its definition and the expansion-made export.
self::written!();

#[macro_export]
macro_rules! written {
    () => {};
}

macro_rules! doc {
    ($i:item) => {
        #[macro_export]
        $i
    };
}

#[cfg(not(doc))]
doc! {
    macro_rules! pick {
        ($a:expr, $b:expr) => { $crate::helper($a) + $b };
    }
}

#[cfg(doc)]
doc! {
    macro_rules! pick {
        () => { compile_error!("doc arm") };
    }
}

pub fn helper(x: u32) -> u32 {
    x * 10
}
