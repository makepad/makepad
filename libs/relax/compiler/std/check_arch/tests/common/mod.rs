use std::mem::{size_of, MaybeUninit};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

// Only called for integer scalars, arrays and the padding-free integer vector types.
unsafe fn sample<T: Copy>(rng: &mut Rng, case: usize, arg: usize) -> T {
    let mut value = MaybeUninit::<T>::uninit();
    let bytes = std::slice::from_raw_parts_mut(value.as_mut_ptr().cast::<u8>(), size_of::<T>());
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = if case < 3 {
            fixture_byte(case, arg, i)
        } else if case < 11 {
            [0, 255, 128, 127, 1, 254, 85, 170][case - 3]
        } else if case < 17 {
            // MIN/MAX and saturation boundaries in each lane width.
            let width = [2, 2, 4, 4, 8, 8][case - 11];
            if (case - 11) % 2 == 0 {
                if i % width == width - 1 {
                    128
                } else {
                    0
                }
            } else if i % width == width - 1 {
                127
            } else {
                255
            }
        } else {
            rng.next() as u8
        };
    }
    value.assume_init()
}

fn fixture_byte(case: usize, arg: usize, i: usize) -> u8 {
    match case {
        0 => 0,
        1 => [
            255, 255, 0, 128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127,
        ][(i + arg * 3) % 16],
        2 => (i * 37 + arg * 71 + 19) as u8,
        _ => unreachable!(),
    }
}

fn bytes<T: Copy>(x: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts((x as *const T).cast::<u8>(), size_of::<T>()) }
}

#[repr(C, align(64))]
#[derive(Copy, Clone)]
struct Buffer([u8; 192]);
