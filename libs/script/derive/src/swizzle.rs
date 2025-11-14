
#![allow(unstable_name_collisions)]

use {
    proc_macro::{
        TokenStream,
    },
    makepad_micro_proc_macro::{TokenBuilder},
};

pub fn pod_swizzle_vec_match_impl(_input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    
    tb.add("match field_name {");
    do_fields(&mut tb, &["x","y","z","w"]);
    do_fields(&mut tb, &["r","g","b","a"]);
    fn do_fields(tb:&mut TokenBuilder, fields:&[&str]){
        // lets generate the vec1 permutations
        for (x, xfield) in fields.iter().enumerate(){
            tb.add("    id!(").ident(xfield).add(")=>self.pod_swizzle_vec1(vt, &pod.data,").unsuf_usize(x).add(", trap),");
        }
        // lets generate the vec2 permutations
        for (x, xfield) in fields.iter().enumerate(){
            for (y, yfield) in fields.iter().enumerate(){
                tb.add("    id!(").ident(&format!("{}{}",xfield,yfield)).add(")=>self.pod_swizzle_vec(vt, &pod.data,[").unsuf_usize(x).add(",").unsuf_usize(y).add("], trap),");
            }
        }
        for (x, xfield) in fields.iter().enumerate(){
            for (y, yfield) in fields.iter().enumerate(){
                for (z, zfield) in fields.iter().enumerate(){
                    tb.add("    id!(").ident(&format!("{}{}{}",xfield,yfield,zfield)).add(")=>self.pod_swizzle_vec(vt, &pod.data,[").unsuf_usize(x).add(",").unsuf_usize(y).add(",").unsuf_usize(z).add("], trap),");
                }
            }
        }
        for (x, xfield) in fields.iter().enumerate(){
            for (y, yfield) in fields.iter().enumerate(){
                for (z, zfield) in fields.iter().enumerate(){
                    for (w, wfield) in fields.iter().enumerate(){
                        tb.add("    id!(").ident(&format!("{}{}{}{}",xfield,yfield,zfield,wfield)).add(")=>self.pod_swizzle_vec(vt, &pod.data,[").unsuf_usize(x).add(",").unsuf_usize(y).add(",").unsuf_usize(z).add(",").unsuf_usize(w).add("], trap),");
                    };
                }
            }
        }
    }
    tb.add("    _=>trap.err_pod_invalid_field_name()");
    tb.add("}");
    tb.end()
}