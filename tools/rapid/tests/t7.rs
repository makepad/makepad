// heap std, Drop, Option/Result and `?`

struct Noisy {
    id: u32,
    log: *mut u64,
}

impl Drop for Noisy {
    fn drop(&mut self) {
        unsafe {
            *self.log = *self.log * 10 + self.id as u64;
        }
    }
}

fn parse_digit(c: u8) -> Result<u32, u32> {
    if c >= b'0' && c <= b'9' {
        Ok((c - b'0') as u32)
    } else {
        Err(c as u32)
    }
}

fn sum_digits(s: &[u8]) -> Result<u32, u32> {
    let mut t = 0;
    for i in 0..s.len() {
        t += parse_digit(s[i])?;
    }
    Ok(t)
}

fn first_even(v: &Vec<i64>) -> Option<i64> {
    for i in 0..v.len() {
        if v[i] % 2 == 0 {
            return Some(v[i]);
        }
    }
    None
}

#[test]
fn vec_basics() {
    let mut v: Vec<i64> = Vec::new();
    for i in 0..1000 {
        v.push(i * 3);
    }
    assert_eq!(v.len(), 1000);
    assert_eq!(v[10], 30);
    v[10] = 7;
    assert_eq!(v[10], 7);
    let mut s = 0i64;
    for x in v.iter() {
        s += *x;
    }
    assert_eq!(s, 1498500 - 30 + 7);
    assert_eq!(v.pop(), Some(2997));
    assert_eq!(first_even(&v), Some(0));
    let b = Box::new(41u32);
    assert_eq!(*b + 1, 42);
}

#[test]
fn strings() {
    let mut s = String::from("hot");
    s.push_str("rust");
    s.push('!');
    assert_eq!(s.len(), 8);
    assert!(s.as_str() == "rapid!");
    let t = "abc".to_string();
    assert!(t.len() == 3);
    println!("{} {}", s, t);
}

#[test]
fn drops_in_order() {
    let mut log = 0u64;
    let lp = &mut log as *mut u64;
    {
        let _a = Noisy { id: 1, log: lp };
        let _b = Noisy { id: 2, log: lp };
        let c = Noisy { id: 3, log: lp };
        drop(c);
        let _d = Box::new(Noisy { id: 4, log: lp });
    }
    // 3 dropped first (explicit), then 4, 2, 1 at scope end in reverse order
    assert_eq!(log, 3421);
    let mut v = Vec::new();
    v.push(Noisy { id: 5, log: lp });
    v.push(Noisy { id: 6, log: lp });
    log = 0;
    drop(v);
    assert_eq!(log, 56);
}

#[test]
fn question_mark() {
    assert_eq!(sum_digits(b"1234"), Ok(10));
    assert_eq!(sum_digits(b"12x4"), Err(120));
}
