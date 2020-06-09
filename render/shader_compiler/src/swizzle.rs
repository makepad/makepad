use crate::ident::Ident;
use std::fmt;
use std::iter::Cloned;
use std::ops::Range;
use std::slice;

#[derive(Clone, Debug)]
pub struct Swizzle {
    indices: Vec<usize>,
}

impl Swizzle {
    pub fn from_range(range: Range<usize>) -> Swizzle {
        println!("POIKA {:?}", range);
        let mut indices = Vec::new();
        for index in range {
            indices.push(index);
        }
        Swizzle { indices }
    }

    pub fn parse(ident: Ident) -> Option<Swizzle> {
        let mut indices = Vec::new();
        ident.with(|string| {
            let mut chars = string.chars();
            let mut ch = chars.next().unwrap();
            match ch {
                'x' | 'y' | 'z' | 'w' => loop {
                    indices.push(match ch {
                        'x' => 0,
                        'y' => 1,
                        'z' => 2,
                        'w' => 3,
                        _ => return None,
                    });
                    ch = match chars.next() {
                        Some(ch) => ch,
                        None => break,
                    };
                },
                'r' | 'g' | 'b' | 'a' => loop {
                    indices.push(match ch {
                        'r' => 0,
                        'g' => 1,
                        'b' => 2,
                        'a' => 3,
                        _ => return None,
                    });
                    ch = match chars.next() {
                        Some(ch) => ch,
                        None => break,
                    };
                },
                _ => return None,
            }
            if indices.len() > 4 {
                return None;
            }
            Some(Swizzle { indices })
        })
    }

    pub fn has_dups(&self) -> bool {
        (0..self.len() - 1).any(|index| self.indices[(index + 1)..].contains(&self.indices[index]))
    }

    pub fn len(&self) -> usize {
        self.indices.len()
    }

    pub fn iter(&self) -> Iter {
        Iter {
            iter: self.indices.iter().cloned(),
        }
    }
}

impl fmt::Display for Swizzle {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for index in &self.indices {
            write!(f, "{}", match index {
                0 => "x",
                1 => "y",
                2 => "z",
                3 => "w",
                _ => panic!(),
            })?;
        }
        Ok(())
    }
}

impl<'a> IntoIterator for &'a Swizzle {
    type Item = usize;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    iter: Cloned<slice::Iter<'a, usize>>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        self.iter.next()
    }
}
