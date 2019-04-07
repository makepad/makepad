use crate::cx::*;
use std::collections::HashMap;
use std::collections::BTreeMap;

// These UI Element containers are the key to automating lifecycle mgmt
// get_draw constructs items that don't exist yet,
// and stores a redraw id. this is used by enumerate.
// If an item is not 'get_draw'ed in a draw pass
// it will be skipped by enumerate/iter.
// However its not destructed and can be get-draw'ed
// in another drawpass.
// if you want to destruct items that werent get_draw'ed
// call sweep on the elements collection.
// If however you can also have 0 items in the collection,
// You HAVE to use mark for sweep to work, since get auto-marks the collection
// Redraw is incremental so there isn't a global 'redraw id' thats
// always the same.
// The idea is to use get_draw in a draw function
// and use the iter/enumerate/get functions in the event handle code
// This does not work for single item Element though

pub trait ElementLife{
    fn construct(&mut self, cx: &mut Cx);
    fn destruct(&mut self, cx: &mut Cx);
}

// Keep a redraw ID with each element 
// to make iterating only 'redrawn in last pass' items possible
#[derive(Clone, Default)]
pub struct ElementsRedraw<T>{
    redraw_id:u64,
    item:T
}

// Multiple elements
#[derive(Clone, Default)]
pub struct Elements<ID, T>
where ID:std::cmp::Ord + std::hash::Hash {
    pub template:T,
    pub element_list:Vec<ID>,
    pub element_map:HashMap<ID,ElementsRedraw<T>>,
    pub redraw_id:u64
}

pub struct ElementsIterator<'a, ID, T>
where ID:std::cmp::Ord + std::hash::Hash{
    elements:&'a mut Elements<ID, T>,
    counter:usize 
}

impl<'a, T, ID> ElementsIterator<'a, ID, T>
where ID:std::cmp::Ord + std::hash::Hash{
    fn new(elements:&'a mut Elements<ID, T>)->Self{
        ElementsIterator{
            elements:elements,
            counter:0
        }
    }
}

impl<'a, ID, T> Iterator for ElementsIterator<'a, ID, T>
where ID:std::cmp::Ord + std::hash::Hash + Clone
{
    type Item = &'a mut T;

    fn next(&mut self) -> Option<Self::Item> {
        loop{
            if self.counter >= self.elements.element_list.len(){
                return None
            }
            let element_id = &self.elements.element_list[self.counter];
            let element = self.elements.element_map.get_mut(&element_id).unwrap();
            self.counter += 1;
            if element.redraw_id == self.elements.redraw_id{
                return Some(unsafe{std::mem::transmute(&mut element.item)});
            }
        }
    }
}


pub struct ElementsIteratorNamed<'a, ID, T>
where ID:std::cmp::Ord + std::hash::Hash {
    elements:&'a mut Elements<ID, T>,
    counter:usize 
}

impl<'a, ID, T> ElementsIteratorNamed<'a, ID, T>
where ID:std::cmp::Ord + std::hash::Hash {
    fn new(elements:&'a mut Elements<ID, T>)->Self{
        ElementsIteratorNamed{
            elements:elements,
            counter:0
        }
    }
}

impl<'a, ID, T> Iterator for ElementsIteratorNamed<'a, ID, T>
where ID:std::cmp::Ord + std::hash::Hash + Clone
{
    type Item = (&'a ID, &'a mut T);

    fn next(&mut self) -> Option<Self::Item> {
        loop{
            if self.counter >= self.elements.element_list.len(){
                return None
            }
            let element_id = &mut self.elements.element_list[self.counter];
            let element = self.elements.element_map.get_mut(&element_id).unwrap();
            self.counter += 1;
            if element.redraw_id == self.elements.redraw_id{
                return Some((unsafe{std::mem::transmute(element_id)}, unsafe{std::mem::transmute(&mut element.item)}));
            }
        }
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

impl<ID,T> Elements<ID, T>
where T:Clone + ElementLife, ID:std::cmp::Ord + std::hash::Hash + Clone
{
    pub fn new(template:T)->Elements<ID, T>{
        Elements::<ID, T>{
            template:template,
            redraw_id:0,
            element_list:Vec::new(),
            element_map:HashMap::new(),
        }
    }

    // if you don't atleast get_draw 1 item
    // you have to call mark for sweep to work
    pub fn mark(&mut self, cx:&Cx){
        if !cx.is_in_redraw_cycle{
            panic!("Cannot call mark outside of redraw cycle!")
        }
        self.redraw_id = cx.redraw_id;
    }

    // destructs all the items that didn't get a mark/get_draw call this time
    pub fn sweep(&mut self, cx:&mut Cx){
        if !cx.is_in_redraw_cycle{
            panic!("Cannot call sweep outside of redraw cycle!")
        }
        let mut i = 0;
        loop{
            if i >= self.element_list.len(){
                break;
            }
            let elem_id = self.element_list[i].clone();
            let elem = self.element_map.get(&elem_id).unwrap();
            if elem.redraw_id != self.redraw_id{
                self.element_list.remove(i);
                let mut elem = self.element_map.remove(&elem_id).unwrap();
                elem.item.destruct(cx);
            }
            else{
                i = i + 1;
            }
        }
    }
    
    // clear all the items
    pub fn clear(&mut self, cx:&mut Cx){
        for elem_id in &self.element_list{
            let mut elem = self.element_map.remove(&elem_id).unwrap();
            elem.item.destruct(cx);
        }
        self.element_list.truncate(0);
    }

    // destruct a particular item
    /*
    pub fn destruct(&mut self, index:ID){
        let elem = self.element_map.get_mut(&index);
        if let Some(elem) = elem{

            self.element_list.find()
            self.element_list.remove()
            return Some(&mut elem.item)
        }
    }*/

    // iterate the set of 'last drawn' items
    pub fn iter<'a>(&'a mut self)->ElementsIterator<'a, ID, T>{
        return ElementsIterator::new(self)
    }

    // enumerate the set of 'last drawn' items
    pub fn enumerate<'a>(&'a mut self)->ElementsIteratorNamed<'a, ID, T>{
        return ElementsIteratorNamed::new(self)
    }

    // gets a particular item. Returns None when not created (yet)
    pub fn get<'a>(&'a mut self, index:ID)->Option<&mut T>{
        let elem = self.element_map.get_mut(&index);
        if let Some(elem) = elem{
            return Some(&mut elem.item)
        }
        else{
            return None
        }
    }
    
    // gets a UI item, if you call it atleast once
    // can be considered as an automatic call to mark
    pub fn get_draw(&mut self, cx: &mut Cx, index:ID)->&mut T{
        if !cx.is_in_redraw_cycle{
            panic!("Cannot call get_draw outside of redraw cycle!")
        }
        self.mark(cx);
        let template = &self.template;
        let element_list = &mut self.element_list;
        let redraw_id = self.redraw_id;
        let redraw = self.element_map.entry(index.clone()).or_insert_with(||{
            element_list.push(index);
            let mut elem = template.clone();
            elem.construct(cx);
            ElementsRedraw{
                redraw_id:redraw_id,
                item:elem
            }
        });
        redraw.redraw_id = redraw_id;
        &mut redraw.item
    }

    pub fn get_draw_or_insert_with<F>(&mut self, cx: &mut Cx, index:ID, mut insert_callback:F)->&mut T
    where F: FnMut(&mut Cx, &T)->T{
        if !cx.is_in_redraw_cycle{
            panic!("Cannot call get_draw outside of redraw cycle!")
        }
        self.mark(cx);
        let template = &self.template;
        let element_list = &mut self.element_list;
        let redraw_id = self.redraw_id;
        let redraw = self.element_map.entry(index.clone()).or_insert_with(||{
            element_list.push(index);
            //let mut elem = template.clone();
            let mut elem = insert_callback(cx, &template);
            elem.construct(cx);
            ElementsRedraw{
                redraw_id:redraw_id,
                item:elem
            }
        });
        redraw.redraw_id = redraw_id;
        &mut redraw.item
    }

/*
    // gets a UI item, if you call it atleast once
    // can be considered as an automatic call to mark
    pub fn get_draw(&mut self, cx: &mut Cx, index:ID)->&mut T{
        if !cx.is_in_redraw_cycle{
            panic!("Cannot call get_draw outside of redraw cycle!")
        }
        self.mark(cx);

        if !self.element_map.contains_key(&index){
            self.element_map.insert(index.clone(), ElementsRedraw{
                redraw_id:self.redraw_id,
                item:self.template.clone()
            });
            self.element_list.push(index.clone());
            let elem = self.element_map.get_mut(&index).unwrap();
            elem.item.construct(cx);
            &mut elem.item
        }
        else{
            let elem = self.element_map.get_mut(&index).unwrap();
            elem.redraw_id = self.redraw_id;
            &mut elem.item
        }
    }

    pub fn get_draw_or_insert<F>(&mut self, cx: &mut Cx, index:ID, mut insert_callback:F)->&mut T
    where F: FnMut(&mut Cx, &mut T){
        if !cx.is_in_redraw_cycle{
            panic!("Cannot call get_draw outside of redraw cycle!")
        }
        self.mark(cx);

        if !self.element_map.contains_key(&index){
            self.element_map.insert(index.clone(), ElementsRedraw{
                redraw_id:self.redraw_id,
                item:self.template.clone()
            });
            self.element_list.push(index.clone());
            let elem = self.element_map.get_mut(&index).unwrap();
            insert_callback(cx, &mut elem.item);
            elem.item.construct(cx);
            &mut elem.item
        }
        else{
            let elem = self.element_map.get_mut(&index).unwrap();
            elem.redraw_id = self.redraw_id;
            &mut elem.item
        }
    }*/

}



// Single element
/*

#[derive(Clone, Default)]
pub struct Element<T>{
    pub template:T,
    pub mark_redraw_id:u64,
    pub get_redraw_id:u64,
    pub element:Option<T>
}

impl<T> Element<T>
where T:Clone + ElementLife
{
    pub fn new(template:T)->Element<T>{
        Element::<T>{
            template:template,
            mark_redraw_id:0,
            get_redraw_id:0,
            element:None
        }
    }

    pub fn get(&mut self)->Option<&mut T>{
        // this is to check if you used mark
        // and didn't use get
        if self.mark_redraw_id != self.get_redraw_id{
            return None;
        }
        return self.element.as_mut()
    }
    
    // if you have 0 or 1 item, you can use mark/sweep
    pub fn mark(&mut self, cx:&Cx){
        if !cx.is_in_redraw_cycle{
            panic!("Cannot call mark outside of redraw cycle!")
        }
        self.mark_redraw_id = cx.redraw_id;
    }

    pub fn sweep(&mut self, cx:&mut Cx){
        if !cx.is_in_redraw_cycle{
            panic!("Cannot call sweep outside of redraw cycle!")
        }
        if !self.element.is_none() && self.mark_redraw_id != self.get_redraw_id{
            let element = self.element.as_mut().unwrap();
            element.destruct(cx);
            self.element = None;
        }
    }

    pub fn get_draw(&mut self, cx:&mut Cx)->&mut T{
        if !cx.is_in_redraw_cycle{
            panic!("Cannot call get_draw outside of redraw cycle!")
        }
        self.mark(cx);
        self.get_redraw_id = cx.redraw_id;
        if self.element.is_none(){
            self.element = Some(self.template.clone());
            let element = self.element.as_mut().unwrap();
            element.construct(cx);
            return element
        }
        let element = self.element.as_mut().unwrap();
        return element
    }
}*/
