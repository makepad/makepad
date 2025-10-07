

#[derive(Default)]
pub struct StringTag(u64);

impl StringTag{
    const MARK:u64 = 0x1;
    const ALLOCED:u64 = 0x2;
        
    pub fn is_alloced(&self)->bool{
        return self.0 & Self::ALLOCED != 0
    }
            
    pub fn set_alloced(&mut self){
        self.0 |= Self::ALLOCED
    }
            
    pub fn clear(&mut self){
        self.0 = 0;
    }
            
    pub fn is_marked(&self)->bool{
        self.0 & Self::MARK != 0
    }
            
    pub fn set_mark(&mut self){
        self.0 |= Self::MARK
    }
            
    pub fn clear_mark(&mut self){
        self.0 &= !Self::MARK
    }
}

#[derive(Default)]
pub struct HeapString{
    pub tag: StringTag,
    pub string: String
}

impl HeapString{
    pub fn clear(&mut self){
        self.tag.clear();
        self.string.clear()
    }
}