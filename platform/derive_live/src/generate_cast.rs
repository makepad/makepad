use proc_macro::TokenStream;

use makepad_macro_lib::{TokenBuilder, TokenParser, error};

pub fn generate_ref_cast_api_impl(input:TokenStream)->TokenStream{

    let mut parser = TokenParser::new(input);
    if let Some(ident) = parser.eat_any_ident(){
        let mut tb = TokenBuilder::new();
        tb.add("impl dyn ").ident(&ident).add(" {");
        tb.add("    pub fn is<T: ").ident(&ident).add(" + 'static >(&self) -> bool {");
        tb.add("        let t = std::any::TypeId::of::<T>();");
        tb.add("        let concrete = self.type_id();");
        tb.add("        t == concrete");
        tb.add("    }");
        tb.add("    pub fn cast<T: ").ident(&ident).add(" + 'static >(&self) -> Option<&T> {");
        tb.add("        if self.is::<T>() {");
        tb.add("            Some(unsafe {&*(self as *const dyn ").ident(&ident).add(" as *const T)})");
        tb.add("        } else {");
        tb.add("            None");
        tb.add("        }");
        tb.add("    }");
        tb.add("    pub fn cast_mut<T: ").ident(&ident).add(" + 'static >(&mut self) -> Option<&mut T> {");
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

pub fn generate_clone_cast_api_impl(input:TokenStream)->TokenStream{

    let mut parser = TokenParser::new(input);
    if let Some(ident) = parser.eat_any_ident(){
        let mut tb = TokenBuilder::new();
        tb.add("impl dyn ").ident(&ident).add(" {");
        tb.add("    pub fn is<T: ").ident(&ident).add(" + 'static >(&self) -> bool {");
        tb.add("        let t = std::any::TypeId::of::<T>();");
        tb.add("        let concrete = self.type_id();");
        tb.add("        t == concrete");
        tb.add("    }");
        tb.add("    pub fn cast<T: ").ident(&ident).add(" + 'static + Default + Clone>(&self) -> T {");
        tb.add("        if self.is::<T>() {");
        tb.add("            unsafe {&*(self as *const dyn ").ident(&ident).add(" as *const T)}.clone()");
        tb.add("        } else {");
        tb.add("            T::default()");
        tb.add("        }");
        tb.add("    }");
        tb.add("}");
        tb.end()
    }
    else{
        error("Expected identifier")
    }
}
