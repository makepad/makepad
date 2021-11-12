use std::fmt;

pub struct CommaSep<'a, T>(pub &'a [T]);

impl<'a, T> fmt::Display for CommaSep<'a, T>
where
    T: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut sep = "";
        for item in self.0 {
            write!(f, "{}{}", sep, item)?;
            sep = ", ";
        }
        Ok(())
    }
}

pub struct PrettyPrintedF32(pub f32);

impl fmt::Display for PrettyPrintedF32 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.0.abs().fract() < 0.00000001 {
            write!(f, "{}.0", self.0)
        } else {
            write!(f, "{}", self.0)
        }
    }
}


pub struct PrettyPrintedF64(pub f64);

impl fmt::Display for PrettyPrintedF64 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.0.abs().fract() < 0.00000001 {
            write!(f, "{}.0", self.0)
        } else {
            write!(f, "{}", self.0)
        }
    }
}

