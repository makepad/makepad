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

