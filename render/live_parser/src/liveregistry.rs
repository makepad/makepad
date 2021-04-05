#![allow(unused_variables)]
use crate::id::{Id,IdFmt};
use crate::liveerror::LiveError;
use makepad_live_derive::*;
use crate::livedocument::LiveDocument;
use crate::livedocument::LiveNodePtr;
use crate::livenode::{LiveNode, LiveValue};

#[derive(Default)]
pub struct LiveRegistry {
    
}

impl LiveRegistry {
    pub fn is_baseclass(&self, id: Id) -> bool {
        id == id!(Component)
    }
    
    pub fn expand_document(&mut self, in_doc: &LiveDocument, errors: &mut Vec<LiveError>) -> LiveDocument {
        
        #[derive(Copy, Clone)]
        enum ScopeTarget {
            Local {node_ptr: LiveNodePtr},
            Use {index: usize}
        }
        
        struct ScopeItem {
            id: Id,
            target: ScopeTarget
        }
        
        struct ScopeStack {
            stack: Vec<Vec<ScopeItem >>
        }
        
        impl ScopeStack {
            fn find_item(&self, id: Id) -> Option<ScopeTarget> {
                if id.is_single() {
                    for items in self.stack.iter().rev() {
                        for item in items.iter().rev() {
                            if item.id == id {
                                return Some(item.target)
                            }
                        }
                    }
                }
                else {
                    
                }
                return None
            }
        }
        
        enum CopyRecurResult {
            IsClass {class: Id},
            IsArray,
            IsObject,
            IsValue,
            IsCall {target: Id},
            Noop,
            Error
        }
        
        fn copy_recur(
            scope_stack: &mut ScopeStack,
            in_doc: Option<&LiveDocument>,
            out_doc: &mut LiveDocument,
            skip_level: usize,
            in_level: usize,
            out_level: usize,
            in_index: usize,
        ) -> CopyRecurResult {
            let node = if let Some(in_doc) = in_doc{
                in_doc.nodes[in_level][in_index]
            }
            else{
                out_doc.nodes[in_level][in_index]
            };
            
            let out_index = out_doc.get_level_len(out_level);
            if in_level == skip_level + 1 && scope_stack.stack.len() - 1 == out_level { // first level, store on scope
                scope_stack.stack[out_level].push(ScopeItem {
                    id: node.id,
                    target: ScopeTarget::Local {node_ptr: LiveNodePtr {level: out_level, index: out_index}}
                });
            }
            
            match node.value {
                LiveValue::Call {target, node_start, node_count} => {
                    let out_start = out_doc.get_level_len(out_level + 1);
                    for i in 0..node_count {
                        copy_recur(scope_stack, in_doc, out_doc, skip_level, in_level + 1, out_level + 1, i as usize + node_start as usize);
                    }
                    if skip_level != in_level {
                        out_doc.push_node(out_level, LiveNode {
                            span: node.span,
                            id: node.id,
                            value: LiveValue::Call {
                                target: target,
                                node_start: out_start as u32,
                                node_count: node_count
                            }
                        });
                    }
                    return CopyRecurResult::IsCall {target}
                },
                LiveValue::Array {node_start, node_count} => {
                    let out_start = out_doc.get_level_len(out_level + 1);
                    for i in 0..node_count {
                        copy_recur(scope_stack, in_doc, out_doc, skip_level, in_level + 1, out_level + 1, i as usize + node_start as usize);
                    }
                    if skip_level != in_level {
                        out_doc.push_node(out_level, LiveNode {
                            span: node.span,
                            id: node.id,
                            value: LiveValue::Array {
                                node_start: out_start as u32,
                                node_count: node_count
                            }
                        });
                    }
                    return CopyRecurResult::IsArray
                },
                LiveValue::Object {node_start, node_count} => {
                    let out_start = out_doc.get_level_len(out_level + 1);
                    for i in 0..node_count {
                        copy_recur(scope_stack, in_doc, out_doc, skip_level, in_level + 1, out_level + 1, i as usize + node_start as usize);
                    }
                    if skip_level != in_level {
                        out_doc.push_node(out_level, LiveNode {
                            span: node.span,
                            id: node.id,
                            value: LiveValue::Object {
                                node_start: out_start as u32,
                                node_count: node_count
                            }
                        });
                    }
                    return CopyRecurResult::IsObject
                },
                LiveValue::Use(_id) => { // shall we output a use in the outupclass?
                }
                LiveValue::Class {class, node_start, node_count} => {
                    if class == id!(Self) {
                        return CopyRecurResult::Noop
                    }
                    let out_start = out_doc.get_level_len(out_level + 1);
                    for i in 0..node_count {
                        copy_recur(scope_stack, in_doc, out_doc, skip_level, in_level + 1, out_level + 1, i as usize + node_start as usize);
                    }
                    if skip_level != in_level {
                        out_doc.push_node(out_level, LiveNode {
                            span: node.span,
                            id: node.id,
                            value: LiveValue::Class {
                                class: class,
                                node_start: out_start as u32,
                                node_count: node_count
                            }
                        });
                    }
                    return CopyRecurResult::IsClass {class}
                },
                _ => {
                    if skip_level != in_level {
                        // we need to use another Id
                        out_doc.push_node(out_level, node);
                    }
                    return CopyRecurResult::IsValue
                }
            }
            return CopyRecurResult::Noop
        }
        
        fn write_or_add_node(scope_stack: &mut ScopeStack, errors: &mut Vec<LiveError>, out_doc: &mut LiveDocument, level: usize, node_start: usize, node_count: usize, in_doc: &LiveDocument, in_node: &LiveNode) {
            match out_doc.write_or_add_node(level, node_start, node_count, in_doc, in_node) {
                Err(err) => errors.push(err),
                Ok(Some(index)) => {
                    if scope_stack.stack.len() - 1 == level {
                        scope_stack.stack[level].push(ScopeItem {
                            id: in_node.id,
                            target: ScopeTarget::Local {node_ptr: LiveNodePtr {level: level, index: index}}
                        });
                    }
                }
                _ => ()
            }
        }
        
        // This should we win me some kind of award. Absolute worst programmer in recent history or something like it.
        fn walk_node(
            registry: &LiveRegistry,
            errors: &mut Vec<LiveError>,
            scope_stack: &mut ScopeStack,
            in_doc: &LiveDocument,
            out_doc: &mut LiveDocument,
            in_level: usize,
            out_level: usize,
            in_node_index: usize,
            out_start: usize,
            out_count: usize
        ) {
            let node = &in_doc.nodes[in_level][in_node_index];
            
            //let (row,col) = byte_to_row_col(node.span.start(), &ld.source);
            //let _ = write!(f, "/*{},{} {}*/", row+1, col, node.span.len());
            match node.value {
                LiveValue::String {string_index, string_len} => {
                },
                LiveValue::Bool(_) => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Int(_) => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Float(_) => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Color(_) => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Vec2(_) => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Vec3(_) => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Id(_val) => write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, node),
                LiveValue::Call {target, node_start, node_count} => {
                    let new_node_start = out_doc.get_level_len(out_level + 1);
                    for i in 0..node_count {
                        walk_node(registry, errors, scope_stack, in_doc, out_doc, in_level + 1, out_level + 1, i as usize + node_start as usize, out_start, 0);
                    }
                    let new_node = LiveNode {
                        span: node.span,
                        id: node.id,
                        value: LiveValue::Call {
                            target,
                            node_start: new_node_start as u32,
                            node_count: node_count
                        }
                    };
                    write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                },
                LiveValue::Array {node_start, node_count} => { // normal array
                    let new_node_start = out_doc.get_level_len(out_level + 1);
                    for i in 0..node_count {
                        walk_node(registry, errors, scope_stack, in_doc, out_doc, in_level + 1, out_level + 1, i as usize + node_start as usize, out_start, 0);
                    }
                    let new_node = LiveNode {
                        span: node.span,
                        id: node.id,
                        value: LiveValue::Array {
                            node_start: new_node_start as u32,
                            node_count: node_count as u32
                        }
                    };
                    write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                },
                LiveValue::Object {node_start, node_count} => {
                    let new_node_start = out_doc.get_level_len(out_level + 1);
                    for i in 0..node_count {
                        walk_node(registry, errors, scope_stack, in_doc, out_doc, in_level + 1, out_level + 1, i as usize + node_start as usize, out_start, 0);
                    }
                    let new_node = LiveNode {
                        span: node.span,
                        id: node.id,
                        value: LiveValue::Object {
                            node_start: new_node_start as u32,
                            node_count: node_count as u32
                        }
                    };
                    write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                },
                LiveValue::Fn {token_start, token_count} => {
                },
                LiveValue::Use(_) => { // shall we output a use in the outupclass?
                    scope_stack.stack[out_level].push(ScopeItem {
                        id: node.id,
                        target: ScopeTarget::Use {index: in_node_index}
                    });
                }
                LiveValue::Class {class, node_start, node_count} => {
                    //let out_index = out_doc.get_level_len(out_level);
                    scope_stack.stack.push(Vec::new());
                    // if our id is a multi-id, write the clone at the correct level
                    let shifted_out_level = if node.id.is_multi() {
                        let (_start, len) = node.id.get_multi();
                        out_level + (len - 1)
                    }
                    else {
                        out_level
                    };
                    
                    let new_out_start = out_doc.get_level_len(shifted_out_level + 1);
                    
                    // result values of the below scan
                    let mut copy_result = CopyRecurResult::IsClass {class};
                    let mut value_ptr = None;
                    
                    if class.is_multi() {
                        let (id_start, id_count) = class.get_multi();
                        let base = in_doc.multi_ids[id_start];
                        // base id can be Self or a scope target
                        if base == id!(Self) {
                            // lets find our sub id chain on self
                            let out_count = out_doc.get_level_len(out_level) - out_start;
                            match out_doc.scan_for_multi(out_level, out_start, out_count, id_start, id_count, &in_doc.multi_ids, node.span) {
                                Ok(found_node) => {
                                    copy_result = copy_recur(scope_stack, None, out_doc, found_node.level, found_node.level, shifted_out_level, found_node.index);
                                    value_ptr = Some(found_node);
                                }
                                Err(err) => {
                                    errors.push(err);
                                    copy_result = CopyRecurResult::Error
                                }
                            }
                        }
                        else if registry.is_baseclass(base) {
                            errors.push(LiveError {
                                span: node.span,
                                message: format!("Setting property {} is not an object path", IdFmt::col(&in_doc.multi_ids, base))
                            });
                            copy_result = CopyRecurResult::Error
                        }
                        else {
                            match scope_stack.find_item(base) {
                                Some(ScopeTarget::Local {node_ptr}) => {
                                    match &out_doc.nodes[node_ptr.level][node_ptr.index].value {
                                        LiveValue::Class {node_start, node_count, ..} => {
                                            match out_doc.scan_for_multi(node_ptr.level + 1, *node_start as usize, *node_count as usize, id_start, id_count, &in_doc.multi_ids, node.span) {
                                                Ok(found_node) => {
                                                    copy_result = copy_recur(scope_stack, None, out_doc, found_node.level, found_node.level, shifted_out_level, found_node.index);
                                                    value_ptr = Some(found_node);
                                                }
                                                Err(err) => {
                                                    errors.push(err);
                                                    copy_result = CopyRecurResult::Error
                                                }
                                            }
                                        }
                                        _ => {
                                            errors.push(LiveError {
                                                span: node.span,
                                                message: format!("Property is not a class {} of {}", base, IdFmt::col(&in_doc.multi_ids, class))
                                            });
                                        }
                                    }
                                }
                                Some(ScopeTarget::Use {index}) => {
                                    // pull in a copy from the use import
                                }
                                None => { // scope item not found, error
                                    errors.push(LiveError {
                                        span: node.span,
                                        message: format!("Cannot find item on scope: {} of {}", base, IdFmt::col(&in_doc.multi_ids, class))
                                    });
                                    copy_result = CopyRecurResult::Error
                                }
                            }
                        }
                    }
                    else if class == id!(Self) {
                        // recursively clone self
                        for i in out_start..out_doc.get_level_len(out_level) {
                            copy_recur(scope_stack, None, out_doc, 0, out_level, shifted_out_level + 1, i);
                        }
                    }
                    else if !registry.is_baseclass(class) {
                        match scope_stack.find_item(class) {
                            Some(ScopeTarget::Local {node_ptr}) => {
                                // first copy the reference here
                                copy_result = copy_recur(scope_stack, None, out_doc, node_ptr.level, node_ptr.level, shifted_out_level, node_ptr.index);
                                value_ptr = Some(node_ptr);
                            }
                            Some(ScopeTarget::Use {index}) => {
                                // pull in a copy from the use import
                            }
                            None => { // scope item not found, error
                                errors.push(LiveError {
                                    span: node.span,
                                    message: format!("Cannot find item on scope: {}", class)
                                });
                                return
                            }
                        }
                    }
                    
                    if let CopyRecurResult::IsClass {..} = copy_result {}
                    else if node_count >0 {
                        errors.push(LiveError {
                            span: node.span,
                            message: format!("Cannot override items in non-class: {}", IdFmt::col(&in_doc.multi_ids, class))
                        });
                    }
                    
                    match copy_result {
                        CopyRecurResult::IsValue => {
                            scope_stack.stack.pop();
                            if let Some(value_ptr) = value_ptr {
                                let mut new_node = out_doc.nodes[value_ptr.level][value_ptr.index];
                                new_node.id = node.id;
                                write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                            }
                        },
                        CopyRecurResult::IsCall {target} => {
                            let new_out_count = out_doc.get_level_len(shifted_out_level + 1) - new_out_start;
                            let new_node = LiveNode {
                                span: node.span,
                                id: node.id,
                                value: LiveValue::Call {
                                    target,
                                    node_start: new_out_start as u32,
                                    node_count: new_out_count as u16
                                }
                            };
                            scope_stack.stack.pop();
                            write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                        },
                        CopyRecurResult::IsArray => {
                            let new_out_count = out_doc.get_level_len(shifted_out_level + 1) - new_out_start;
                            let new_node = LiveNode {
                                span: node.span,
                                id: node.id,
                                value: LiveValue::Array {
                                    node_start: new_out_start as u32,
                                    node_count: new_out_count as u32
                                }
                            };
                            scope_stack.stack.pop();
                            write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                        },
                        CopyRecurResult::IsObject => {
                            let new_out_count = out_doc.get_level_len(shifted_out_level + 1) - new_out_start;
                            let new_node = LiveNode {
                                span: node.span,
                                id: node.id,
                                value: LiveValue::Object {
                                    node_start: new_out_start as u32,
                                    node_count: new_out_count as u32
                                }
                            };
                            scope_stack.stack.pop();
                            write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                        },
                        CopyRecurResult::IsClass {class} => {
                            let new_out_count = out_doc.get_level_len(shifted_out_level + 1) - new_out_start;
                            for i in 0..node_count {
                                walk_node(registry, errors, scope_stack, in_doc, out_doc, in_level + 1, shifted_out_level + 1, i as usize + node_start as usize, new_out_start, new_out_count);
                            }
                            let new_out_count = out_doc.get_level_len(shifted_out_level + 1) - new_out_start;
                            
                            let new_node = LiveNode {
                                span: node.span,
                                id: node.id,
                                value: LiveValue::Class {
                                    class,
                                    node_start: new_out_start as u32,
                                    node_count: new_out_count as u16
                                }
                            };
                            scope_stack.stack.pop();
                            write_or_add_node(scope_stack, errors, out_doc, out_level, out_start, out_count, in_doc, &new_node);
                        }
                        _ => {
                            scope_stack.stack.pop();
                        }
                        
                    }
                }
            }
        }
        
        let mut out = LiveDocument::new();
        let mut scope_stack = ScopeStack {
            stack: vec![Vec::new()]
        };
        let len = in_doc.nodes[0].len();
        
        for i in 0..len {
            walk_node(self, errors, &mut scope_stack, in_doc, &mut out, 0, 0, i, 0, 0);
        }
        
        out
        // ok lets walk our tree
        // and clone it into the output ld
    }
}
