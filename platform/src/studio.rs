
// communication enums for studio
pub enum AppToStudio{
    Log{
        body:String,
        file: u32,
        line: u32
    }
}

pub enum StudioToApp{
    LiveChange{
        file_name: String,
        content: String
    }
}
