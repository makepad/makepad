
#![allow(unstable_name_collisions)]

use {
    proc_macro::{
        TokenStream,
    },
    makepad_micro_proc_macro::{TokenBuilder},
};

pub fn pod_swizzle_vec_type_impl(_input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
        
    tb.add("match field_name {");
    do_fields(&mut tb, &["x","y","z","w"]);
    do_fields(&mut tb, &["r","g","b","a"]);
    fn do_fields(tb:&mut TokenBuilder, fields:&[&str]){
        // lets generate the vec1 permutations
        for (_x, xfield) in fields.iter().enumerate(){
            tb.add("    id!(").ident(xfield).add(")=>Some(vt.swizzle_type(1, builtins)),");
        }
        // lets generate the vec2 permutations
        for (_x, xfield) in fields.iter().enumerate(){
            for (_y, yfield) in fields.iter().enumerate(){
                tb.add("    id!(").ident(&format!("{}{}",xfield,yfield)).add(")=>Some(vt.swizzle_type(2, builtins)),");
            }
        }
        for (_x, xfield) in fields.iter().enumerate(){
            for (_y, yfield) in fields.iter().enumerate(){
                for (_z, zfield) in fields.iter().enumerate(){
                    tb.add("    id!(").ident(&format!("{}{}{}",xfield,yfield,zfield)).add(")=>Some(vt.swizzle_type(3, builtins)),");
                }
            }
        }
        for (_x, xfield) in fields.iter().enumerate(){
            for (_y, yfield) in fields.iter().enumerate(){
                for (_z, zfield) in fields.iter().enumerate(){
                    for (_w, wfield) in fields.iter().enumerate(){
                        tb.add("    id!(").ident(&format!("{}{}{}{}",xfield,yfield,zfield,wfield)).add(")=>Some(vt.swizzle_type(4, builtins)),");
                    };
                }
            }
        }
    }
    tb.add("    _=>None");
    tb.add("}");
    tb.end()
}
    

pub fn pod_swizzle_vec_match_impl(_input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    
    tb.add("match field_name {");
    do_fields(&mut tb, &["x","y","z","w"]);
    do_fields(&mut tb, &["r","g","b","a"]);
    fn do_fields(tb:&mut TokenBuilder, fields:&[&str]){
        // lets generate the vec1 permutations
        for (x, xfield) in fields.iter().enumerate(){
            tb.add("    id!(").ident(xfield).add(")=>self.pod_swizzle_vec1(*vt, data,").unsuf_usize(x).add(", trap),");
        }
        // lets generate the vec2 permutations
        for (x, xfield) in fields.iter().enumerate(){
            for (y, yfield) in fields.iter().enumerate(){
                tb.add("    id!(").ident(&format!("{}{}",xfield,yfield)).add(")=>self.pod_swizzle_vec(*vt, data,[").unsuf_usize(x).add(",").unsuf_usize(y).add("], builtins, trap),");
            }
        }
        for (x, xfield) in fields.iter().enumerate(){
            for (y, yfield) in fields.iter().enumerate(){
                for (z, zfield) in fields.iter().enumerate(){
                    tb.add("    id!(").ident(&format!("{}{}{}",xfield,yfield,zfield)).add(")=>self.pod_swizzle_vec(*vt, data,[").unsuf_usize(x).add(",").unsuf_usize(y).add(",").unsuf_usize(z).add("], builtins, trap),");
                }
            }
        }
        for (x, xfield) in fields.iter().enumerate(){
            for (y, yfield) in fields.iter().enumerate(){
                for (z, zfield) in fields.iter().enumerate(){
                    for (w, wfield) in fields.iter().enumerate(){
                        tb.add("    id!(").ident(&format!("{}{}{}{}",xfield,yfield,zfield,wfield)).add(")=>self.pod_swizzle_vec(*vt, data,[").unsuf_usize(x).add(",").unsuf_usize(y).add(",").unsuf_usize(z).add(",").unsuf_usize(w).add("], builtins, trap),");
                    };
                }
            }
        }
    }
    tb.add("    _=>script_err_pod!(trap, \"unknown swizzle field\")");
    tb.add("}");
    tb.end()
}