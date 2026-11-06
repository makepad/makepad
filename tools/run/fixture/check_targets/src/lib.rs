//! check_targets.sh: each target of the package has a lint of its own, so the warnings name the
//! targets a check selection reached.

pub struct Model(pub u32);

impl Model {
    pub fn evaluate(&self, e: &mut u32) -> u32 {
        *e += 1;
        self.0 + *e
    }
}

// used by the lib's unit tests only: dead in the plain lib
fn only_tests_use_it() -> u32 {
    2
}

#[cfg(test)]
mod tests {
    #[test]
    fn evaluates() {
        let mut e = 0;
        let mut m = super::Model(1);
        assert_eq!(m.evaluate(&mut e) + super::only_tests_use_it(), 4);
    }
}
