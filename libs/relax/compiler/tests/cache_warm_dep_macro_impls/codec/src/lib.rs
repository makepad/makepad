// One macro writes an impl per array length, each length a call of a `const fn`: every impl's
// length is an anonymous constant of its own impl. They shared one cache identity, so a `test`
// and a `run` of the same program on one cache resolved the method on `[u8; 3]` to another
// length's impl, or found none (relax PR 158, relax issue 142).
pub trait Encode {
    fn encode(&self) -> Vec<u8>;
}

const fn len(n: usize) -> usize {
    let mut k = 0;
    while k < n {
        k += 1;
    }
    k
}

macro_rules! encode_arrays {
    ($($n:expr),*) => {$(
        impl Encode for [u8; len($n)] {
            fn encode(&self) -> Vec<u8> {
                let mut v = self.to_vec();
                v.push($n);
                v
            }
        }
    )*};
}
encode_arrays!(1, 2, 3, 4);
