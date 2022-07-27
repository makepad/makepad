use {
    std::{
        fmt::Write,
        ops::Deref,
        ops::DerefMut,
        iter
    },
    crate::{
        makepad_error_log::*, 
        makepad_derive_live::{
            live_object
        },
        makepad_math::{
            Vec2,
            Vec3,
            Vec4
        },
        makepad_live_tokenizer::LiveId,
        live_token::LiveTokenId,
        live_node::{LivePropType, LiveNode, LiveValue, LiveNodeOrigin, InlineString, FittedString, LiveProp},
    }
};

pub trait LiveNodeSlice {
    fn parent(&self, child_index: usize) -> Option<usize>;
    fn append_child_index(&self, parent_index: usize) -> usize;
    fn first_child(&self, parent_index: usize) -> Option<usize>;
    fn last_child(&self, parent_index: usize) -> Option<usize>;
    fn next_child(&self, child_index: usize) -> Option<usize>;
    fn node_slice(&self, parent_index: usize) -> &[LiveNode];
    fn children_slice(&self, parent_index: usize) -> &[LiveNode];
    
    fn child_by_number(&self, parent_index: usize, child_number: usize) -> Option<usize>;
    fn child_or_append_index_by_name(&self, parent_index: usize, name: LiveProp) -> Result<usize, usize>;
    //fn next_child_by_name(&self, child_index: usize, name: LiveId) -> Option<usize>;
    fn child_by_name(&self, parent_index: usize, name: LiveProp) -> Option<usize>;
    fn sibling_by_name(&self, start_index: usize, name: LiveProp) -> Option<usize>;
    fn child_by_path(&self, parent_index: usize, path: &[LiveProp]) -> Option<usize>;
    fn child_value_by_path(&self, parent_index: usize, path: &[LiveProp]) -> Option<&LiveValue>;
    fn read_path(&self, path: &str) -> Option<&LiveValue>;
    
    fn first_node_with_token_id(&self, match_token_id: LiveTokenId, also_in_dsl: bool) -> Option<usize>;
    
    fn get_num_sibling_nodes(&self, child_index: usize) -> usize;
    
    fn scope_up_by_name(&self, parent_index: usize, name: LiveProp) -> Option<usize>;
    fn scope_up_down_by_name(&self, parent_index: usize, name: LiveProp) -> Option<usize>;
    
    fn count_children(&self, parent_index: usize) -> usize;
    fn skip_node(&self, node_index: usize) -> usize;
    fn clone_child(&self, parent_index: usize, out_vec: &mut Vec<LiveNode>);
    fn to_string(&self, parent_index: usize, max_depth: usize) -> String;
    fn debug_print(&self, parent_index: usize, max_depth: usize);
}

pub trait LiveNodeVec {
    fn insert_node_from_other(&mut self, from_index: usize, insert_start: usize, other: &[LiveNode]) -> usize;
    fn insert_node_from_self(&mut self, from_index: usize, insert_start: usize) -> usize;
    
    fn insert_children_from_other(&mut self, from_index: usize, insert_start: usize, other: &[LiveNode]);
    fn insert_children_from_self(&mut self, from_index: usize, insert_start: usize);
    
    fn write_path(&mut self, path: &str, value: LiveValue);
    fn replace_or_insert_last_node_by_path(&mut self, start_index: usize, path: &[LiveProp], other: &[LiveNode]);
    fn replace_or_insert_first_node_by_path(&mut self, start_index: usize, path: &[LiveProp], other: &[LiveNode]);
    
    fn push_live(&mut self, v: &[LiveNode]);
    fn push_str(&mut self, id: LiveId, v: &'static str);
    fn push_string(&mut self, id: LiveId, v: &str);
    fn push_bool(&mut self, id: LiveId, v: bool);
    fn push_int(&mut self, id: LiveId, v: i64);
    fn push_float(&mut self, id: LiveId, v: f64);
    fn push_color(&mut self, id: LiveId, v: u32);
    fn push_vec2(&mut self, id: LiveId, v: Vec2);
    fn push_vec3(&mut self, id: LiveId, v: Vec3);
    fn push_vec4(&mut self, id: LiveId, v: Vec4);
    fn push_id(&mut self, id: LiveId, v: LiveId);
    fn push_bare_enum(&mut self, id: LiveId, base: LiveId, variant: LiveId);
    
    fn open_tuple_enum(&mut self, id: LiveId, base: LiveId, variant: LiveId);
    fn open_named_enum(&mut self, id: LiveId, base: LiveId, variant: LiveId);
    fn open_object(&mut self, id: LiveId);
    fn open_clone(&mut self, id: LiveId, clone: LiveId);
    fn open_array(&mut self, id: LiveId);
    
    fn open(&mut self);
    fn close(&mut self);
}

// accessing the Gen structure like a tree
impl<T> LiveNodeSlice for T where T: AsRef<[LiveNode]> {
    
    fn first_node_with_token_id(&self, match_token_id: LiveTokenId, also_in_dsl: bool) -> Option<usize> {
        for (node_index, node) in self.as_ref().iter().enumerate() {
            if let Some(token_id) = node.origin.token_id() {
                if token_id == match_token_id {
                    return Some(node_index)
                }
                // lets see if we are a DSL node then match the token range
                if also_in_dsl && token_id.file_id() == match_token_id.file_id() {
                    match node.value {
                        LiveValue::DSL {token_start, token_count, ..} => {
                            let index = match_token_id.token_index() as u32;
                            if index >= token_start && index <= token_start + token_count {
                                return Some(node_index);
                            }
                        }
                        _ => ()
                    }
                }
            }
            // we might have to do a range scan
            
        }
        None
    }
    
    fn parent(&self, child_index: usize) -> Option<usize> {
        let self_ref = self.as_ref();
        if self_ref.len() == 0 {
            return None
        }
        let mut stack_depth = 0;
        let mut index = child_index - 1;
        // we are going to scan backwards
        loop {
            if self_ref[index].is_open() {
                if stack_depth == 0 {
                    return Some(index)
                }
                stack_depth -= 1;
            }
            else if self_ref[index].is_close() {
                stack_depth += 1;
            }
            if index == 0 {
                break
            }
            index -= 1;
        }
        Some(0)
    }
    
    fn get_num_sibling_nodes(&self, start_index: usize) -> usize {
        let self_ref = self.as_ref();
        if self_ref.len() == 0 {
            return 0
        }
        let mut stack_depth: isize = 0;
        let mut index = start_index;
        // scan backwards to find a node with this name
        loop {
            if self_ref[index].is_open() {
                if stack_depth>0 {
                    stack_depth -= 1;
                }
                else {
                    return start_index - index;
                }
            }
            else if self_ref[index].is_close() {
                stack_depth += 1;
            }
            if index == 0 {
                break
            }
            index -= 1;
        }
        0
    }
    
    fn scope_up_by_name(&self, index: usize, name: LiveProp) -> Option<usize> {
        let self_ref = self.as_ref();
        if self_ref.len() == 0 {
            return None
        }
        let mut stack_depth: isize = 0;
        let mut index = index;
        // scan backwards to find a node with this name
        loop {
            if self_ref[index].is_open() {
                if stack_depth>0 {
                    stack_depth -= 1;
                }
            }
            else if self_ref[index].is_close() {
                stack_depth += 1;
            }
            if stack_depth == 0 && self_ref[index].id == name.0 && self_ref[index].origin.has_prop_type(name.1) && !self_ref[index].is_close() { // valuenode
                return Some(index)
            }
            
            if index == 0 {
                break
            }
            index -= 1;
        }
        None
    }
    
    fn scope_up_down_by_name(&self, start_index: usize, name: LiveProp) -> Option<usize> {
        let self_ref = self.as_ref();
        if self_ref.len() == 0 {
            return None
        }
        let mut stack_depth: isize = 0;
        let mut index = start_index;
        // scan backwards to find a node with this name
        loop {
            if self_ref[index].is_open() {
                if stack_depth == 0 {
                    if let Some(child_index) = self.child_by_name(index, name) {
                        if child_index != start_index {
                            return Some(child_index)
                        }
                    }
                }
                if stack_depth>0 {
                    stack_depth -= 1;
                }
            }
            else if self_ref[index].is_close() {
                stack_depth += 1;
            }
            /*
            if stack_depth == 0 {
                
                if self_ref[index].id == name && index != start_index && !self_ref[index].value.is_close() { // valuenode
                    return Some(index)
                }
                // scan child down
                if self_ref[index].is_open() {
                    if let Some(child_index) = self.child_by_name(index, name) {
                        if child_index != start_index {
                            return Some(child_index)
                        }
                    }
                }
            }*/
            if index == 0 {
                break
            }
            index -= 1;
        }
        None
    }
    
    
    fn child_by_number(&self, parent_index: usize, child_number: usize) -> Option<usize> {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        let mut child_count = 0;
        if !self_ref[index].is_open() {
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].is_open() {
                if stack_depth == 1 {
                    if child_number == child_count {
                        return Some(index);
                    }
                    child_count += 1;
                }
                stack_depth += 1;
            }
            else if self_ref[index].is_close() {
                stack_depth -= 1;
                if stack_depth == 0 {
                    return None
                }
            }
            else {
                if stack_depth == 1 {
                    if child_number == child_count {
                        return Some(index);
                    }
                    child_count += 1;
                }
                else if stack_depth == 0 {
                    panic!()
                }
            }
            index += 1;
        }
        panic!()
    }
    
    fn first_child(&self, parent_index: usize) -> Option<usize> {
        let self_ref = self.as_ref();
        if self_ref[parent_index].is_open() {
            if self_ref[parent_index + 1].is_close() {
                return None
            }
            return Some(parent_index + 1) // our first child
        }
        panic!()
    }
    
    fn next_child(&self, child_index: usize) -> Option<usize> {
        let self_ref = self.as_ref();
        let mut index = child_index;
        let mut stack_depth = 0;
        while index < self_ref.len() {
            if self_ref[index].is_open() {
                if stack_depth == 0 && index != child_index {
                    return Some(index)
                }
                stack_depth += 1;
            }
            else if self_ref[index].is_close() {
                if stack_depth == 0 { // double close
                    return None;
                }
                stack_depth -= 1;
            }
            else {
                if stack_depth == 0 && index != child_index {
                    return Some(index)
                }
            }
            index += 1;
        }
        None
    }
    
    fn last_child(&self, parent_index: usize) -> Option<usize> {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        //let mut child_count = 0;
        let mut found_child = None;
        if !self_ref[index].is_open() {
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].is_open() {
                if stack_depth == 1 {
                    found_child = Some(index);
                    //child_count += 1;
                }
                stack_depth += 1;
            }
            else if self_ref[index].is_close() {
                stack_depth -= 1;
                if stack_depth == 0 {
                    return found_child
                }
            }
            else {
                if stack_depth == 1 {
                    found_child = Some(index);
                    //child_count += 1;
                }
                else if stack_depth == 0 {
                    panic!()
                }
            }
            index += 1;
        }
        None
    }
    
    fn node_slice(&self, start_index: usize) -> &[LiveNode] {
        let next_index = self.skip_node(start_index);
        &self.as_ref()[start_index..next_index]
    }
    
    fn children_slice(&self, start_index: usize) -> &[LiveNode] {
        if !self.as_ref()[start_index].is_open() {
            &self.as_ref()[start_index..start_index]
        }
        else {
            let next_index = self.as_ref().skip_node(start_index);
            &self.as_ref()[start_index + 1..next_index - 1]
        }
    }
    
    
    fn append_child_index(&self, parent_index: usize) -> usize {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        if !self_ref[index].is_open() {
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].is_open() {
                stack_depth += 1;
            }
            else if self_ref[index].is_close() {
                stack_depth -= 1;
                if stack_depth == 0 {
                    return index
                }
            }
            index += 1;
        }
        index
    }
    
    fn child_or_append_index_by_name(&self, parent_index: usize, child_name: LiveProp) -> Result<usize, usize> {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        if !self_ref[index].is_open() {
            return Err(index)
            //panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].is_open() {
                if stack_depth == 1 {
                    if self_ref[index].origin.has_prop_type(child_name.1) && self_ref[index].id == child_name.0 {
                        return Ok(index);
                    }
                }
                stack_depth += 1;
            }
            else if self_ref[index].is_close() {
                stack_depth -= 1;
                if stack_depth == 0 {
                    return Err(index)
                }
            }
            else {
                if stack_depth == 1 {
                    if self_ref[index].origin.has_prop_type(child_name.1) && self_ref[index].id == child_name.0 {
                        return Ok(index);
                    }
                }
                if stack_depth == 0 {
                    panic!()
                }
            }
            index += 1;
        }
        Err(index)
    }
    
    fn child_by_name(&self, parent_index: usize, name: LiveProp) -> Option<usize> {
        if let Ok(value) = self.child_or_append_index_by_name(parent_index, name) {
            Some(value)
        }
        else {
            None
        }
    }
    /*
    fn sibling_by_name(&self, start_index: usize, name: LiveId) -> Option<usize>{
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = start_index;
        while index < self_ref.len() {
            if self_ref[index].is_open() {
                if stack_depth == 0 {
                    if !self_ref[index].origin.id_non_unique() && self_ref[index].id == name {
                        return Some(index);
                    }
                }
                stack_depth += 1;
            }
            else if self_ref[index].is_close() {
                if stack_depth == 0 {
                    return None
                }
                stack_depth -= 1;
            }
            else {
                if stack_depth == 0 {
                    if !self_ref[index].origin.id_non_unique() && self_ref[index].id == name {
                        return Some(index);
                    }
                }
            }
            index += 1;
        }
        None
    }*/
    
    
    fn sibling_by_name(&self, child_index: usize, child_name: LiveProp) -> Option<usize> {
        let self_ref = self.as_ref();
        let mut stack_depth = 1;
        let mut index = child_index;
        while index < self_ref.len() {
            if self_ref[index].is_open() {
                if stack_depth == 1 {
                    if self_ref[index].id == child_name.0 {
                        return Some(index);
                    }
                }
                stack_depth += 1;
            }
            else if self_ref[index].is_close() {
                stack_depth -= 1;
                if stack_depth == 0 {
                    return None
                }
            }
            else {
                if stack_depth == 1 {
                    if !self_ref[index].origin.has_prop_type(child_name.1) && self_ref[index].id == child_name.0 {
                        return Some(index);
                    }
                }
                if stack_depth == 0 {
                    panic!()
                }
            }
            index += 1;
        }
        None
    }
    
    
    fn child_by_path(&self, parent_index: usize, path: &[LiveProp]) -> Option<usize> {
        let mut index = parent_index;
        for level in path {
            if let Some(child) = self.child_by_name(index, *level) {
                index = child
            }
            else {
                return None
            }
        }
        Some(index)
    }
    
    fn child_value_by_path(&self, parent_index: usize, path: &[LiveProp]) -> Option<&LiveValue> {
        if let Some(index) = self.child_by_path(parent_index, path) {
            Some(&self.as_ref()[index].value)
        }
        else {
            None
        }
    }
    
    fn read_path(&self, path: &str) -> Option<&LiveValue> {
        let mut ids = [LiveProp(LiveId(0), LivePropType::Field); 8];
        let mut parsed = 0;
        for (index, step) in path.split(".").enumerate() {
            if parsed >= ids.len() {
                eprintln!("read_path too many path segs");
                return None
            }
            ids[index] = LiveProp(LiveId::from_str_unchecked(step), LivePropType::Field);
            parsed += 1;
        }
        if let Some(index) = self.child_by_path(0, &ids[0..parsed]) {
            Some(&self.as_ref()[index].value)
        }
        else {
            None
        }
    }
    
    fn count_children(&self, parent_index: usize) -> usize {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        let mut count = 0;
        if !self_ref[index].is_open() {
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].is_open() {
                if stack_depth == 1 {
                    count += 1;
                }
                stack_depth += 1;
            }
            else if self_ref[index].is_close() {
                stack_depth -= 1;
                if stack_depth == 0 {
                    return count
                }
            }
            else {
                if stack_depth == 1 {
                    count += 1;
                }
                else if stack_depth == 0 {
                    panic!()
                }
            }
            index += 1;
        }
        panic!()
    }
    
    fn skip_node(&self, node_index: usize) -> usize {
        let self_ref = self.as_ref();
        let mut index = node_index;
        let mut stack_depth = 0;
        while index < self_ref.len() {
            if self_ref[index].is_open() {
                stack_depth += 1;
            }
            else if self_ref[index].is_close() {
                if stack_depth == 0 {
                    panic!()
                }
                stack_depth -= 1;
                if stack_depth == 0 {
                    index += 1;
                    return index
                }
            }
            else {
                if stack_depth == 0 {
                    index += 1;
                    return index
                }
            }
            index += 1;
        }
        return index
    }
    
    fn clone_child(&self, parent_index: usize, out: &mut Vec<LiveNode>) {
        let self_ref = self.as_ref();
        let mut index = parent_index;
        let mut stack_depth = 0;
        while index < self_ref.len() {
            out.push(self_ref[index].clone());
            if self_ref[index].is_open() {
                stack_depth += 1;
            }
            else if self_ref[index].is_close() {
                stack_depth -= 1;
                if stack_depth == 0 {
                    return
                }
            }
            else {
                if stack_depth == 0 {
                    return
                }
            }
            index += 1;
        }
        return
    }
    
    fn debug_print(&self, parent_index: usize, max_depth: usize) {
        log!("{}", self.to_string(parent_index, max_depth));
    }
    
    fn to_string(&self, parent_index: usize, max_depth: usize) -> String {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut f = String::new();
        let mut index = parent_index;
        while index < self_ref.len() {
            let node = &self_ref[index];
            if stack_depth > max_depth {
                if node.is_open() {
                    stack_depth += 1;
                }
                else if node.is_close() {
                    stack_depth -= 1;
                }
                index += 1;
                continue
            }
            for _ in 0..stack_depth {
                write!(f, "|   ").unwrap();
            }
            let pt = match node.origin.prop_type() {
                LivePropType::Field => ":",
                LivePropType::Instance => "=",
                LivePropType::Template => "=?",
                LivePropType::Nameless => "??"
            };
            match &node.value {
                LiveValue::None => {
                    writeln!(f, "{}{} <None>", node.id, pt).unwrap();
                },
                LiveValue::Str(s) => {
                    writeln!(f, "{}{} <Str> {}", node.id, pt, s).unwrap();
                },
                LiveValue::InlineString(s) => {
                    writeln!(f, "{}{} <InlineString> {}", node.id, pt, s.as_str()).unwrap();
                },
                LiveValue::FittedString(s) => {
                    writeln!(f, "{}{} <FittedString> {}", node.id, pt, s.as_str()).unwrap();
                },
                LiveValue::DocumentString {string_start, string_count} => {
                    writeln!(f, "{}{} <DocumentString> string_start:{}, string_end:{}", node.id, pt, string_start, string_count).unwrap();
                },
                LiveValue::Dependency {string_start, string_count} => {
                    writeln!(f, "{}{} <Dependency> string_start:{}, string_end:{}", node.id, pt, string_start, string_count).unwrap();
                },
                LiveValue::Bool(v) => {
                    writeln!(f, "{}{} <Bool> {}", node.id, pt, v).unwrap();
                }
                LiveValue::Int(v) => {
                    writeln!(f, "{}{} <Int> {}", node.id, pt, v).unwrap();
                }
                LiveValue::Float(v) => {
                    writeln!(f, "{}{} <Float> {}", node.id, pt, v).unwrap();
                },
                LiveValue::Color(v) => {
                    writeln!(f, "{}{} <Color>{:08x}", node.id, pt, v).unwrap();
                },
                LiveValue::Vec2(v) => {
                    writeln!(f, "{}{} <Vec2> {:?}", node.id, pt, v).unwrap();
                },
                LiveValue::Vec3(v) => {
                    writeln!(f, "{}{} <Vec3> {:?}", node.id, pt, v).unwrap();
                },
                LiveValue::Vec4(v) => {
                    writeln!(f, "{}{} <Vec4> {:?}", node.id, pt, v).unwrap();
                },
                LiveValue::Id(id) => {
                    writeln!(f, "{}{} <Id> {}", node.id, pt, id).unwrap();
                },
                LiveValue::ExprBinOp(id) => {
                    writeln!(f, "{}{} <ExprBinOp> {:?}", node.id, pt, id).unwrap();
                },
                LiveValue::ExprUnOp(id) => {
                    writeln!(f, "{}{} <ExprUnOp> {:?}", node.id, pt, id).unwrap();
                },
                LiveValue::ExprMember(id) => {
                    writeln!(f, "{}{} <ExprMember> {:?}", node.id, pt, id).unwrap();
                },
                LiveValue::BareEnum {base, variant} => {
                    writeln!(f, "{}{} <BareEnum> {}::{}", node.id, pt, base, variant).unwrap();
                },
                // stack items
                LiveValue::Expr {expand_index} => {
                    writeln!(f, "{}{} <Expr> {:?}", node.id, pt, expand_index).unwrap();
                    stack_depth += 1;
                },
                LiveValue::ExprCall {ident, args} => {
                    writeln!(f, "{}{} <ExprCall> {}({})", node.id, pt, ident, args).unwrap();
                },
                LiveValue::Array => {
                    writeln!(f, "{}{} <Array>", node.id, pt).unwrap();
                    stack_depth += 1;
                },
                LiveValue::TupleEnum {base, variant} => {
                    writeln!(f, "{}{} <TupleEnum> {}::{}", node.id, pt, base, variant).unwrap();
                    stack_depth += 1;
                },
                LiveValue::NamedEnum {base, variant} => {
                    writeln!(f, "{}{} <NamedEnum> {}::{}", node.id, pt, base, variant).unwrap();
                    stack_depth += 1;
                },
                LiveValue::Object => {
                    writeln!(f, "{}{} <Object>", node.id, pt).unwrap();
                    stack_depth += 1;
                }, // subnodes including this one
                LiveValue::Clone(clone) => {
                    writeln!(f, "{}{} <Clone> {}", node.id, pt, clone).unwrap();
                    stack_depth += 1;
                }, // subnodes including this one
                LiveValue::Class {live_type, ..} => {
                    writeln!(f, "{}{} <Class> {:?}", node.id, pt, live_type).unwrap();
                    stack_depth += 1;
                }, // subnodes including this one
                LiveValue::Close => {
                    if stack_depth == 0 {
                        writeln!(f, "<CloseMisaligned> {}", node.id).unwrap();
                        break;
                    }
                    writeln!(f, "<Close> {}", node.id).unwrap();
                    stack_depth -= 1;
                    if stack_depth == 0 {
                        break;
                    }
                },
                // the shader code types
                LiveValue::DSL {
                    token_start,
                    token_count,
                    expand_index
                } => {
                    writeln!(f, "<DSL> {} :token_start:{}, token_count:{} expand_index:{:?}", node.id, token_start, token_count, expand_index).unwrap();
                },
                LiveValue::Import(module_path) => {
                    writeln!(f, "<Import> {}::{}", module_path, node.id).unwrap();
                }
                LiveValue::Registry(component_id) => {
                    writeln!(f, "<Registry> {}::{}", component_id, node.id).unwrap();
                }
            }
            index += 1;
        }
        if stack_depth != 0 {
            writeln!(f, "[[ERROR Stackdepth not 0 at end {}]]", stack_depth).unwrap()
        }
        f
    }
}
//}
/*
impl_live_node_slice!(&[LiveNode]);
impl_live_node_slice!(&mut [LiveNode]);
impl_live_node_slice!(Vec<LiveNode>);
*/


impl LiveNodeVec for Vec<LiveNode> {
    
    fn insert_children_from_other(&mut self, source_index: usize, insert_point: usize, other: &[LiveNode]) {
        
        if !other[source_index].is_open() {
            panic!();
        }
        let next_source = other.skip_node(source_index);
        let num_children = (next_source - source_index) - 2;
        self.splice(insert_point..insert_point, other[source_index + 1..(source_index + 1 + num_children)].iter().cloned());
    }
    
    
    fn insert_children_from_self(&mut self, source_index: usize, insert_point: usize) {
        // get the # of children here
        if !self[source_index].is_open() {
            panic!();
        }
        let next_source = self.skip_node(source_index);
        let num_children = (next_source - source_index) - 2;
        
        self.splice(insert_point..insert_point, iter::repeat(LiveNode::empty()).take(num_children));
        
        let source_index = if insert_point < source_index {source_index + num_children}else {source_index};
        
        for i in 0..num_children {
            self[insert_point + i] = self[source_index + 1 + i].clone();
        }
    }
    
    
    fn insert_node_from_self(&mut self, source_index: usize, insert_point: usize) -> usize {
        let next_source = self.skip_node(source_index);
        let num_nodes = next_source - source_index;
        // make space
        self.splice(insert_point..insert_point, iter::repeat(LiveNode::empty()).take(num_nodes));
        
        let source_index = if insert_point < source_index {source_index + num_nodes}else {source_index};
        
        for i in 0..num_nodes {
            self[insert_point + i] = self[source_index + i].clone();
        }
        
        insert_point + num_nodes
    }
    
    fn insert_node_from_other(&mut self, source_index: usize, insert_point: usize, other: &[LiveNode]) -> usize {
        let next_source = other.skip_node(source_index);
        let num_nodes = next_source - source_index;
        // make space
        self.splice(insert_point..insert_point, other[source_index..(source_index + num_nodes)].iter().cloned());
        
        insert_point + num_nodes
    }
    
    fn write_path(&mut self, path: &str, value: LiveValue) {
        let mut ids = [LiveProp(LiveId(0), LivePropType::Field); 8];
        let mut parsed = 0;
        for (index, step) in path.split(".").enumerate() {
            if parsed >= ids.len() {
                eprintln!("write_path too many path segs");
                return
            }
            ids[index] = LiveProp(LiveId::from_str_unchecked(step), LivePropType::Field);
            parsed += 1;
        }
        let was_empty = self.len() == 0;
        if was_empty{
            self.open();
        }
        self.replace_or_insert_last_node_by_path(
            0,
            &ids[0..parsed],
            &[LiveNode::from_value(value)]
        );
        if was_empty{
            self.close();
        }
    }
    
    fn replace_or_insert_last_node_by_path(&mut self, start_index: usize, path: &[LiveProp], other: &[LiveNode]) {
        let mut index = start_index;
        let mut depth = 0;
        while depth < path.len() {
            match self.child_or_append_index_by_name(index, path[depth]) {
                Ok(found_index) => {
                    index = found_index;
                    if depth == path.len() - 1 { // last
                        let next_index = self.skip_node(found_index);
                        self.splice(found_index..next_index, other.iter().cloned());
                        // overwrite id
                        self[found_index].origin.set_prop_type(path[depth].1);
                        self[found_index].id = path[depth].0;
                        return
                    }
                }
                Err(append_index) => {
                    index = append_index;
                    if depth == path.len() - 1 { // last
                        self.splice(append_index..append_index, other.iter().cloned());
                        // lets overwrite the id
                        self[append_index].origin.set_prop_type(path[depth].1);
                        self[append_index].id = path[depth].0;
                        return
                    }
                    else { // insert an empty object
                        self.splice(append_index..append_index, live_object!{
                            [path[depth].0]: {}
                        }.iter().cloned());
                        self[append_index].origin.set_prop_type(path[depth].1);
                    }
                }
            }
            depth += 1;
        }
    }
    
    fn replace_or_insert_first_node_by_path(&mut self, start_index: usize, path: &[LiveProp], other: &[LiveNode]) {
        let mut index = start_index;
        let mut depth = 0;
        while depth < path.len() {
            match self.child_by_name(index, path[depth]) {
                Some(found_index) => {
                    index = found_index;
                    if depth == path.len() - 1 { // last
                        let next_index = self.skip_node(found_index);
                        self.splice(found_index..next_index, other.iter().cloned());
                        // overwrite id
                        self[found_index].id = path[depth].0;
                        return
                    }
                }
                None => {
                    index = index + 1;
                    if depth == path.len() - 1 { // last
                        self.splice(index..index, other.iter().cloned());
                        // lets overwrite the id
                        self[index].id = path[depth].0;
                        self[index].origin = LiveNodeOrigin::empty().with_prop_type(path[depth].1);
                        return
                    }
                    else { // insert an empty object
                        self.splice(index..index, live_object!{
                            [path[depth].0]: {}
                        }.iter().cloned());
                        self[index].origin = LiveNodeOrigin::empty().with_prop_type(path[depth].1);
                    }
                }
            }
            depth += 1;
        }
    }
    
    
    fn push_live(&mut self, v: &[LiveNode]) {self.extend_from_slice(v)}
    
    fn push_str(&mut self, id: LiveId, v: &'static str) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::Str(v)})}
    fn push_string(&mut self, id: LiveId, v: &str) {
        //let bytes = v.as_bytes();
        if let Some(inline_str) = InlineString::from_str(v) {
            self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::InlineString(inline_str)});
        }
        else {
            self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::FittedString(FittedString::from_string(v.to_string()))});
        }
    }
    
    fn push_bool(&mut self, id: LiveId, v: bool) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::Bool(v)})}
    fn push_int(&mut self, id: LiveId, v: i64) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::Int(v)})}
    fn push_float(&mut self, id: LiveId, v: f64) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::Float(v)})}
    fn push_color(&mut self, id: LiveId, v: u32) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::Color(v)})}
    fn push_vec2(&mut self, id: LiveId, v: Vec2) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::Vec2(v)})}
    fn push_vec3(&mut self, id: LiveId, v: Vec3) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::Vec3(v)})}
    fn push_vec4(&mut self, id: LiveId, v: Vec4) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::Vec4(v)})}
    fn push_id(&mut self, id: LiveId, v: LiveId) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::Id(v)})}
    
    fn push_bare_enum(&mut self, id: LiveId, base: LiveId, variant: LiveId) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::BareEnum {base, variant}})}
    fn open_tuple_enum(&mut self, id: LiveId, base: LiveId, variant: LiveId) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::TupleEnum {base, variant}})}
    fn open_named_enum(&mut self, id: LiveId, base: LiveId, variant: LiveId) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::NamedEnum {base, variant}})}
    fn open_object(&mut self, id: LiveId) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::Object})}
    fn open_clone(&mut self, id: LiveId, clone: LiveId) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::Clone(clone)})}
    fn open_array(&mut self, id: LiveId) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id, value: LiveValue::Array})}
    fn close(&mut self) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id: LiveId(0), value: LiveValue::Close})}
    fn open(&mut self) {self.push(LiveNode {origin: LiveNodeOrigin::empty(), id: LiveId(0), value: LiveValue::Object})}
}
const MAX_CLONE_STACK_DEPTH_SAFETY: usize = 100;

pub struct LiveNodeReader<'a> {
    eot: bool,
    depth: usize,
    index: usize,
    nodes: &'a[LiveNode]
}

impl<'a> LiveNodeReader<'a> {
    pub fn new(index: usize, nodes: &'a[LiveNode]) -> Self {
        
        Self {
            eot: false,
            depth: 0,
            index,
            nodes
        }
    }
    
    pub fn index_option(&self, index: Option<usize>, depth_change: isize) -> Option<Self> {
        if self.eot {panic!();}
        if let Some(index) = index {
            Some(Self {
                eot: self.eot,
                depth: (self.depth as isize + depth_change) as usize,
                index: index,
                nodes: self.nodes
            })
        }
        else {
            None
        }
    }
    
    pub fn node(&self) -> &LiveNode {
        if self.eot {panic!();}
        &self.nodes[self.index]
    }
    
    pub fn parent(&self) -> Option<Self> {self.index_option(self.nodes.parent(self.index), -1)}
    pub fn append_child_index(&self) -> usize {self.nodes.append_child_index(self.index)}
    pub fn first_child(&self) -> Option<Self> {self.index_option(self.nodes.first_child(self.index), 1)}
    pub fn last_child(&self) -> Option<Self> {self.index_option(self.nodes.last_child(self.index), 1)}
    pub fn next_child(&self) -> Option<Self> {self.index_option(self.nodes.next_child(self.index), 0)}
    
    pub fn node_slice(&self) -> &[LiveNode] {
        if self.eot {panic!()}
        self.nodes.node_slice(self.index)
    }
    
    pub fn children_slice(&self) -> &[LiveNode] {
        if self.eot {panic!()}
        self.nodes.children_slice(self.index)
    }
    
    pub fn child_by_number(&self, child_number: usize) -> Option<Self> {
        self.index_option(self.nodes.child_by_number(self.index, child_number), 1)
    }
    
    pub fn child_by_name(&self, name: LiveProp) -> Option<Self> {
        self.index_option(self.nodes.child_by_name(self.index, name), 1)
    }
    
    fn child_by_path(&self, path: &[LiveProp]) -> Option<Self> {
        self.index_option(self.nodes.child_by_path(self.index, path), 1)
    }
    
    pub fn scope_up_by_name(&self, name: LiveProp) -> Option<Self> {
        self.index_option(self.nodes.scope_up_by_name(self.index, name), 0)
    }
    
    pub fn count_children(&self) -> usize {self.nodes.count_children(self.index)}
    pub fn clone_child(&self, out_vec: &mut Vec<LiveNode>) {
        if self.eot {panic!();}
        self.nodes.clone_child(self.index, out_vec)
    }
    
    pub fn to_string(&self, max_depth: usize) -> String {
        if self.eot {panic!();}
        self.nodes.to_string(self.index, max_depth)
    }
    
    pub fn skip(&mut self) {
        if self.eot {panic!();}
        self.index = self.nodes.skip_node(self.index);
        // check eot
        if self.nodes[self.index].is_close() { // standing on a close node
            if self.depth == 1 {
                self.eot = true;
                self.index += 1;
            }
        }
    }
    
    pub fn walk(&mut self) {
        if self.eot {panic!();}
        if self.nodes[self.index].is_open() {
            self.depth += 1;
        }
        else if self.nodes[self.index].is_close() {
            if self.depth == 0 {panic!()}
            self.depth -= 1;
            if self.depth == 0 {
                self.eot = true;
            }
        }
        self.index += 1;
    }
    
    pub fn is_eot(&self) -> bool {
        return self.eot
    }
    
    pub fn index(&self) -> usize {
        self.index
    }
    
    pub fn depth(&self) -> usize {
        self.depth
    }
    
    pub fn nodes(&self) -> &[LiveNode] {
        self.nodes
    }
    
}

impl<'a> Deref for LiveNodeReader<'a> {
    type Target = LiveNode;
    fn deref(&self) -> &Self::Target {&self.nodes[self.index]}
}


pub struct LiveNodeMutReader<'a> {
    eot: bool,
    depth: usize,
    index: usize,
    nodes: &'a mut [LiveNode]
}

impl<'a> LiveNodeMutReader<'a> {
    pub fn new(index: usize, nodes: &'a mut [LiveNode]) -> Self {
        Self {
            eot: false,
            depth: 0,
            index,
            nodes
        }
    }
    
    pub fn node(&mut self) -> &mut LiveNode {
        if self.eot {panic!();}
        &mut self.nodes[self.index]
    }
    
    pub fn node_slice(&self) -> &[LiveNode] {
        if self.eot {panic!()}
        self.nodes.node_slice(self.index)
    }
    
    pub fn children_slice(&self) -> &[LiveNode] {
        if self.eot {panic!()}
        self.nodes.children_slice(self.index)
    }
    
    pub fn count_children(&self) -> usize {self.nodes.count_children(self.index)}
    
    pub fn clone_child(&self, out_vec: &mut Vec<LiveNode>) {
        if self.eot {panic!();}
        self.nodes.clone_child(self.index, out_vec)
    }
    
    pub fn to_string(&self, max_depth: usize) -> String {
        if self.eot {panic!();}
        self.nodes.to_string(self.index, max_depth)
    }
    
    pub fn skip(&mut self) {
        if self.eot {panic!();}
        self.index = self.nodes.skip_node(self.index);
        if self.nodes[self.index].is_close() { // standing on a close node
            if self.depth == 1 {
                self.eot = true;
                self.index += 1;
            }
        }
    }
    
    pub fn walk(&mut self) {
        if self.eot {panic!();}
        if self.nodes[self.index].is_open() {
            self.depth += 1;
        }
        else if self.nodes[self.index].value.is_close() {
            if self.depth == 0 {panic!()}
            self.depth -= 1;
            if self.depth == 0 {
                self.eot = true;
            }
        }
        self.index += 1;
    }
    
    pub fn is_eot(&mut self) -> bool {
        return self.eot
    }
    
    pub fn index(&mut self) -> usize {
        self.index
    }
    
    pub fn depth(&mut self) -> usize {
        self.depth
    }
    
    pub fn nodes(&mut self) -> &mut [LiveNode] {
        self.nodes
    }
    
}

impl<'a> Deref for LiveNodeMutReader<'a> {
    type Target = LiveNode;
    fn deref(&self) -> &Self::Target {&self.nodes[self.index]}
}

impl<'a> DerefMut for LiveNodeMutReader<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.nodes[self.index]}
}