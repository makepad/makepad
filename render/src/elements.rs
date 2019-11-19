use crate::cx::*;
use std::collections::HashMap;
//use std::collections::BTreeMap;

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
// Keep a redraw ID with each element 
// to make iterating only 'redrawn in last pass' items possible
#[derive(Clone, Default)]
pub struct ElementsRedraw<T>{
    redraw_id:u64,
    item:T
}

// Multiple elements
#[derive(Clone, Default)]
pub struct Elements<ID, T, TEMPL>
where ID:std::cmp::Ord + std::hash::Hash {
    pub template:TEMPL,
    pub element_list:Vec<ID>,
    pub element_map:HashMap<ID,ElementsRedraw<T>>,
    pub redraw_id:u64
}

pub struct ElementsIterator<'a, ID, T, TEMPL>
where ID:std::cmp::Ord + std::hash::Hash{
    elements:&'a mut Elements<ID, T, TEMPL>,
    counter:usize 
}

impl<'a, T, ID, TEMPL> ElementsIterator<'a, ID, T, TEMPL>
where ID:std::cmp::Ord + std::hash::Hash{
    fn new(elements:&'a mut Elements<ID, T, TEMPL>)->Self{
        ElementsIterator{
            elements:elements,
            counter:0
        }
    }
}

impl<'a, ID, T, TEMPL> Iterator for ElementsIterator<'a, ID, T, TEMPL>
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


pub struct ElementsIteratorNamed<'a, ID, T, TEMPL>
where ID:std::cmp::Ord + std::hash::Hash{
    elements:&'a mut Elements<ID, T, TEMPL>,
    counter:usize 
}

impl<'a, ID, T, TEMPL> ElementsIteratorNamed<'a, ID, T, TEMPL>
where ID:std::cmp::Ord + std::hash::Hash{
    fn new(elements:&'a mut Elements<ID, T, TEMPL>)->Self{
        ElementsIteratorNamed{
            elements:elements,
            counter:0
        }
    }
}

impl<'a, ID, T, TEMPL> Iterator for ElementsIteratorNamed<'a, ID, T, TEMPL>
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

impl<ID,T, TEMPL> Elements<ID, T, TEMPL>
where ID:std::cmp::Ord + std::hash::Hash + Clone
{
    pub fn new(template:TEMPL)->Elements<ID, T, TEMPL>{
        Elements::<ID, T, TEMPL>{
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
    pub fn sweep<F>(&mut self, cx:&mut Cx, mut destruct_callback:F)
    where F: FnMut(&mut Cx, &mut T){
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
                destruct_callback(cx, &mut elem.item);
            }
            else{
                i = i + 1;
            }
        }
    }
    
    // clear all the items
    pub fn clear<F>(&mut self, cx:&mut Cx, mut destruct_callback:F)
    where F: FnMut(&mut Cx, &mut T){
        for elem_id in &self.element_list{
            let mut elem = self.element_map.remove(&elem_id).unwrap();
            destruct_callback(cx, &mut elem.item);
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
    pub fn iter<'a>(&'a mut self)->ElementsIterator<'a, ID, T, TEMPL>{
        return ElementsIterator::new(self)
    }

    // enumerate the set of 'last drawn' items
    pub fn enumerate<'a>(&'a mut self)->ElementsIteratorNamed<'a, ID, T, TEMPL>{
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
   
    pub fn get_draw<F>(&mut self, cx: &mut Cx, index:ID, mut insert_callback:F)->&mut T
    where F: FnMut(&mut Cx, &TEMPL)->T{
        if !cx.is_in_redraw_cycle{
            panic!("Cannot call get_draw outside of redraw cycle!")
        }
        self.mark(cx);
        let template = &self.template;
        let element_list = &mut self.element_list;
        let redraw_id = self.redraw_id;
        let redraw = self.element_map.entry(index.clone()).or_insert_with(||{
            element_list.push(index);
            let elem = insert_callback(cx, &template);
            ElementsRedraw{
                redraw_id:redraw_id,
                item:elem
            }
        });
        redraw.redraw_id = redraw_id;
        &mut redraw.item
    }
}