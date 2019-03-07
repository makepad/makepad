pub use derive_element::*;
use crate::cx::*;
use std::collections::BTreeMap;

pub trait ElementLife{
    fn construct(&mut self, cx: &mut Cx);
    fn destruct(&mut self, cx: &mut Cx);
    fn update(&mut self, cx: &mut Cx);
}


// Multiple elements
#[derive(Clone, Default)]
pub struct Elements<T, ID>
where ID:std::cmp::Ord {
    pub template:T,
    pub element_list:Vec<(bool,ID)>,
    pub element_map:BTreeMap<ID,T>,
    pub len:usize
}

pub struct ElementsIterator<'a, T, ID>
where ID:std::cmp::Ord {
    elements:&'a mut Elements<T, ID>,
    counter:usize 
}

impl<'a, T, ID> ElementsIterator<'a, T, ID>
where ID:std::cmp::Ord {
    fn new(elements:&'a mut Elements<T, ID>)->Self{
        ElementsIterator{
            elements:elements,
            counter:0
        }
    }
}

impl<'a, T, ID> Iterator for ElementsIterator<'a, T, ID>
where ID:std::cmp::Ord + Clone
{
    type Item = &'a mut T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.counter >= self.elements.element_list.len(){
            return None
        }
        let element_id = &self.elements.element_list[self.counter].1;
        let element = self.elements.element_map.get_mut(&element_id).unwrap();
        self.counter += 1;
        return Some(unsafe{std::mem::transmute(element)});
    }
}


pub struct ElementsIteratorNamed<'a, T, ID>
where ID:std::cmp::Ord {
    elements:&'a mut Elements<T, ID>,
    counter:usize 
}

impl<'a, T, ID> ElementsIteratorNamed<'a, T, ID>
where ID:std::cmp::Ord {
    fn new(elements:&'a mut Elements<T, ID>)->Self{
        ElementsIteratorNamed{
            elements:elements,
            counter:0
        }
    }
}

impl<'a, T, ID> Iterator for ElementsIteratorNamed<'a, T, ID>
where ID:std::cmp::Ord + Clone
{
    type Item = (&'a ID, &'a mut T);

    fn next(&mut self) -> Option<Self::Item> {
        if self.counter >= self.elements.element_list.len(){
            return None
        }
        let element_id = &self.elements.element_list[self.counter].1;
        let element = self.elements.element_map.get_mut(&element_id).unwrap();
        self.counter += 1;
        return Some((unsafe{std::mem::transmute(element_id)}, unsafe{std::mem::transmute(element)}));
    }
}

/*
// and we'll implement IntoIterator
impl<'a, T, ID> IntoIterator for &'a mut Elements<T,ID>
where ID:std::cmp::Ord + Clone
{
    type Item = &'a mut T;
    type IntoIter = ElementsIterator<'a,T,ID>;

    fn into_iter(self) -> Self::IntoIter {
        ElementsIterator::new(self)
    }
}*/

impl<T,ID> Elements<T,ID>
where T:Clone + ElementLife, ID:std::cmp::Ord + Clone
{
    pub fn new(template:T)->Elements<T,ID>{
        Elements::<T,ID>{
            template:template,
            element_list:Vec::new(),
            element_map:BTreeMap::new(),
            len:0
        }
    }

    // gc'ing your element sets
    pub fn mark(&mut self){
        for (mark, _elem_id) in &mut self.element_list{
            *mark = true;
        }
    }

    pub fn sweep(&mut self, cx:&mut Cx){
        let mut i = 0;
        loop{
            if i >= self.element_list.len(){
                break;
            }
            if self.element_list[i].0{
                let elem_id = self.element_list[i].1.clone();
                self.element_list.remove(i);
                let mut elem = self.element_map.remove(&elem_id).unwrap();
                elem.destruct(cx);
            }
            else{
                i = i + 1;
            }
        }
    }
    
    pub fn all<'a>(&'a mut self)->ElementsIterator<'a, T, ID>{
        return ElementsIterator::new(self)
    }

    pub fn ids<'a>(&'a mut self)->ElementsIteratorNamed<'a, T, ID>{
        return ElementsIteratorNamed::new(self)
    }

    pub fn get(&mut self, cx: &mut Cx, index:ID)->&mut T{
        if !self.element_map.contains_key(&index){
            self.element_map.insert(index.clone(), self.template.clone());
            self.element_list.push((false, index.clone()));
            let elem = self.element_map.get_mut(&index).unwrap();
            elem.construct(cx);
            elem
        }
        else{
            let elem = self.element_map.get_mut(&index).unwrap();
            elem.update(cx);
            elem
        }
    }
}



// Single element


#[derive(Clone, Default)]
pub struct Element<T>{
    pub template:T,
    pub redraw_id:u64,
    pub element:Option<T>
}

pub struct ElementIterator<'a, T>{
    element:&'a mut Element<T>,
    done:bool 
}

impl<'a, T> ElementIterator<'a, T>{
    fn new(element:&'a mut Element<T>)->Self{
        ElementIterator{
            element:element,
            done:false
        }
    }
}

impl<'a, T> Iterator for ElementIterator<'a, T>
{
    type Item = &'a mut T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done{
            return None;
        }
        self.done = true;
        let element = self.element.element.as_mut();
        if element.is_none(){
            return None
        }
        return Some(unsafe{std::mem::transmute(element.unwrap())});
    }
}
/*
// and we'll implement IntoIterator
impl<'a, T> IntoIterator for &'a mut Element<T>
{
    type Item = &'a mut T;
    type IntoIter = ElementIterator<'a,T>;

    fn into_iter(self) -> Self::IntoIter {
        ElementIterator::new(self)
    }
}
*/
impl<T> Element<T>
where T:Clone + ElementLife
{
    pub fn new(template:T)->Element<T>{
        Element::<T>{
            template:template,
            redraw_id:0,
            element:None
        }
    }

    pub fn all<'a>(&'a mut self)->ElementIterator<'a,T>{
        return ElementIterator::new(self)
    }
 
    pub fn get(&mut self, cx:&mut Cx)->&mut T{
        if self.redraw_id == cx.redraw_id{
            cx.log("WARNING Item is called multiple times in a single drawpass!\n");
        }
        self.redraw_id = cx.redraw_id;
        if self.element.is_none(){
            self.element = Some(self.template.clone());
            let element = self.element.as_mut().unwrap();
            element.construct(cx);
            return element
        }
        let element = self.element.as_mut().unwrap();
        element.update(cx);
        return element
    }
}
