const MOD: u32 = 65521;
const NMAX: usize = 5552;

pub fn update(a: u16, b: u16, data: &[u8]) -> (u16, u16) {
  let mut a = a as u32;
  let mut b = b as u32;

  let chunks = data.chunks_exact(NMAX);
  let remainder = chunks.remainder();

  for chunk in chunks {
    for byte in chunk {
      a = a.wrapping_add(*byte as _);
      b = b.wrapping_add(a);
    }

    a %= MOD;
    b %= MOD;
  }

  for byte in remainder {
    a = a.wrapping_add(*byte as _);
    b = b.wrapping_add(a);
  }

  a %= MOD;
  b %= MOD;

  (a as u16, b as u16)
}

#[cfg(test)]
mod tests {
  #[test]
  fn zeroes_short() {
    assert_eq!(adler32(&[]), 1);
    assert_eq!(adler32(&[0]), 1 | 1 << 16);
    assert_eq!(adler32(&[0, 0]), 1 | 2 << 16);
    assert_eq!(adler32(&[0; 100]), 0x00640001);
    assert_eq!(adler32(&[0; 1024]), 0x04000001);
  }

  #[test]
  fn zeroes_long() {
    // Cross the checksum modulus while processing full chunks and a remainder.
    assert_eq!(adler32(&[0; 65522]), 0x00010001);
  }

  #[test]
  fn ones_short() {
    assert_eq!(adler32(&[0xff; 1024]), 0x79a6fc2e);
  }

  #[test]
  fn ones_long() {
    // Two full 5552-byte chunks and a remainder exercise repeated reduction.
    assert_eq!(adler32(&[0xff; 11105]), 0x37a33825);
  }

  #[test]
  fn mixed_short() {
    assert_eq!(adler32(&[1]), 2 | 2 << 16);
    assert_eq!(adler32(&[40]), 41 | 41 << 16);
  }

  #[test]
  fn mixed_long() {
    // Two full 5552-byte chunks and a remainder exercise repeated reduction.
    assert_eq!(adler32(&[0xA5; 11105]), 0x607af71b);
  }

  /// Example calculation from https://en.wikipedia.org/wiki/Adler-32.
  #[test]
  fn wiki() {
    assert_eq!(adler32(b"Wikipedia"), 0x11E60398);
  }

  fn adler32(data: &[u8]) -> u32 {
    let (a, b) = super::update(1, 0, data);

    u32::from(b) << 16 | u32::from(a)
  }
}
