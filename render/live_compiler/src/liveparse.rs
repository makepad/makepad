 
#[derive(Clone, Hash, PartialEq, Eq, Debug)]
pub struct LiveBody {
    pub file: String,
    pub module_path: String,
    pub line: usize,
    pub column: usize,
    pub body: String
}


