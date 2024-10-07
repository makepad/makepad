use proc_macro::TokenStream;

use makepad_micro_proc_macro::{TokenBuilder, TokenParser, error};

pub fn generate_any_send_trait_api_impl(input:TokenStream)->TokenStream{

    let mut parser = TokenParser::new(input);
    if let Some(ident) = parser.eat_any_ident(){
        let mut tb = TokenBuilder::new();
        tb.add("impl dyn ").ident(&ident).add(" + Send + Sync{");
        tb.add("    pub fn is<T: ").ident(&ident).add(" + 'static + Send + Sync >(&self) -> bool {");
        tb.add("        let t = std::any::TypeId::of::<T>();");
        tb.add("        let concrete = self.ref_cast_type_id();");
        tb.add("        t == concrete");
        tb.add("    }");
        tb.add("    pub fn downcast_ref<T: ").ident(&ident).add(" + 'static + Send + Sync>(&self) -> Option<&T> {");
        tb.add("        if self.is::<T>() {");
        tb.add("            Some(unsafe {&*(self as *const dyn ").ident(&ident).add(" as *const T)})");
        tb.add("        } else {");
        tb.add("            None");
        tb.add("        }");
        tb.add("    }");
        tb.add("    pub fn downcast_mut<T: ").ident(&ident).add(" + 'static + Send + Sync>(&mut self) -> Option<&mut T> {");
        tb.add("        if self.is::<T>() {");
        tb.add("            Some(unsafe {&mut *(self as *const dyn ").ident(&ident).add(" as *mut T)})");
        tb.add("        } else {");
        tb.add("            None");
        tb.add("        }");
        tb.add("    }");
        tb.add("}");
        tb.end()
    }
    else{
        error("Expected identifier")
    }
}

pub fn generate_any_trait_api_impl(input:TokenStream)->TokenStream{
    
    let mut parser = TokenParser::new(input);
    if let Some(ident) = parser.eat_any_ident(){
        let mut tb = TokenBuilder::new();
        tb.add("impl dyn ").ident(&ident).add(" {");
        tb.add("    pub fn is<T: ").ident(&ident).add(" + 'static >(&self) -> bool {");
        tb.add("        let t = std::any::TypeId::of::<T>();");
        tb.add("        let concrete = self.ref_cast_type_id();");
        tb.add("        t == concrete");
        tb.add("    }");
        tb.add("    pub fn downcast_ref<T: ").ident(&ident).add(" + 'static >(&self) -> Option<&T> {");
        tb.add("        if self.is::<T>() {");
        tb.add("            Some(unsafe {&*(self as *const dyn ").ident(&ident).add(" as *const T)})");
        tb.add("        } else {");
        tb.add("            None");
        tb.add("        }");
        tb.add("    }");
        tb.add("    pub fn downcast_mut<T: ").ident(&ident).add(" + 'static >(&mut self) -> Option<&mut T> {");
        tb.add("        if self.is::<T>() {");
        tb.add("            Some(unsafe {&mut *(self as *const dyn ").ident(&ident).add(" as *mut T)})");
        tb.add("        } else {");
        tb.add("            None");
        tb.add("        }");
        tb.add("    }");
        tb.add("}");
        tb.end()
    }
    else{
        error("Expected identifier")
    }
}
