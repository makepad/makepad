
pub struct Elements<T>{
    pub template:T,
    pub elements:Vec<T>,
    pub len:usize
}

impl<T> Elements<T>
where T:Clone
{
    pub fn new(template:T)->Elements<T>{
        Elements::<T>{
            template:template,
            elements:Vec::new(),
            len:0
        }
    }
    pub fn len(&self)->usize{
        self.len
    }

    pub fn reset(&mut self){
        self.len = 0;
    }

    pub fn add(&mut self)->&mut T{
        if self.len >= self.elements.len(){
            self.elements.push(self.template.clone());
            self.len += 1;
            self.elements.last_mut().unwrap()

        }
        else{
            let last = self.len;
            self.len += 1;
            &mut self.elements[last]
        }
    }
}
