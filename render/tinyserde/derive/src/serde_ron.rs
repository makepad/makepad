use crate::shared::*;
use proc_macro2::{TokenStream};
use syn::{
    parse_quote,
    Ident,
    DeriveInput,
    Fields,
    FieldsNamed,
    FieldsUnnamed,
    DataEnum,
    LitInt,
    LitStr,
    Type,
};
use quote::quote;
use quote::format_ident;
use syn::spanned::Spanned;

pub fn derive_ser_ron_struct(input: &DeriveInput, fields: &FieldsNamed) -> TokenStream {
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let bound = parse_quote!(SerRon);
    let bounded_where_clause = where_clause_with_bound(&input.generics, bound);
    let ident = &input.ident;

    let mut outputs = Vec::new();
    for field in &fields.named {
        let fieldname = field.ident.clone().unwrap();
        let fieldstring = LitStr::new(&fieldname.to_string(), ident.span());
        if type_is_option(&field.ty) {
            outputs.push(quote! {if let Some(t) = self.#fieldname {s.field(d+1,#fieldstring);t.ser_ron(d+1, s);s.conl();};})
        }
        else {
            outputs.push(quote! {s.field(d+1,#fieldstring);self.#fieldname.ser_ron(d+1, s);s.conl();})
        }
    }

    quote!{
        impl #impl_generics SerRon for #ident #ty_generics #bounded_where_clause {
            fn ser_ron(&self, d: usize, s: &mut makepad_tinyserde::SerRonState) {
                s.st_pre();
                #(
                    #outputs
                ) *
                s.st_post(d);
            }
        }
    }
}

pub fn type_is_option(ty: &Type) -> bool {
    if let Type::Path(tp) = ty {
        if tp.path.segments.len() == 1 && tp.path.segments[0].ident.to_string() == "Option" {
            return true;
        }
    }
    return false
}


pub fn derive_de_ron_named(ident:TokenStream, fields: &FieldsNamed) -> TokenStream {
    let mut local_vars = Vec::new();
    let mut field_names = Vec::new();
    let mut field_strings = Vec::new();
    let mut unwraps = Vec::new();
    for field in &fields.named {
         
        let fieldname = field.ident.clone().unwrap();
        let localvar = format_ident!("_{}", fieldname);
        let fieldstring = LitStr::new(&fieldname.to_string(), ident.span());
        
        if type_is_option(&field.ty) {
            unwraps.push(quote! {if let Some(t) = #localvar {t}else {None}})
        }
        else {
            unwraps.push(quote! {if let Some(t) = #localvar {t}else {return Err(s.err_nf(#fieldstring))}})
        }
        
        field_names.push(fieldname);
        local_vars.push(localvar);
        field_strings.push(fieldstring);
    }
    
    quote!{
        #(
            let mut #local_vars = None;
        ) *
        s.paren_open(i) ?;
        while let Some(_) = s.next_ident() {
            match s.identbuf.as_ref() {
                #(
                    #field_strings => {s.next_colon(i) ?;#local_vars = Some(DeRon::de_ron(s, i) ?)},
                ) *
                _ => return std::result::Result::Err(s.err_exp(&s.identbuf))
            }
            s.eat_comma_paren(i) ?
        }
        s.paren_close(i) ?;
        #ident {#(
            #field_names: #unwraps,
        ) *}
    }
}

pub fn derive_de_ron_struct(input: &DeriveInput, fields: &FieldsNamed) -> TokenStream {
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let bound = parse_quote!(DeRon);
    let bounded_where_clause = where_clause_with_bound(&input.generics, bound);
    let ident = &input.ident;
    let body = derive_de_ron_named(quote!{#ident}, fields);
    
    quote!{
        impl #impl_generics DeRon for #ident #ty_generics #bounded_where_clause {
            fn de_ron(s: &mut makepad_tinyserde::DeRonState, i: &mut std::str::Chars) -> std::result::Result<Self,
            DeRonErr> {
                std::result::Result::Ok({#body})
            }
        }
    }
} 

pub fn derive_ser_ron_enum(input: &DeriveInput, enumeration: &DataEnum) -> TokenStream {
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let bound = parse_quote!(SerRon);
    let bounded_where_clause = where_clause_with_bound(&input.generics, bound);
    
    let ident = &input.ident;
    
    let mut match_item = Vec::new();
    
    for variant in &enumeration.variants {
        let ident = &variant.ident;
        let lit = LitStr::new(&ident.to_string(), ident.span());
        match &variant.fields {
            Fields::Unit => {
                match_item.push(quote!{
                    Self::#ident => s.out.push_str(#lit),
                })
            },
            Fields::Named(fields_named) => {
                let mut items = Vec::new();
                let mut field_names = Vec::new();
                for field in &fields_named.named {
                    if let Some(field_name) = &field.ident {
                        let field_string = LitStr::new(&field_name.to_string(), field_name.span());
                        if type_is_option(&field.ty) {
                            items.push(quote!{
                                if #field_name.is_some(){
                                    s.field(d+1, #field_string);
                                    #field_name.ser_ron(d+1, s);
                                    s.conl();
                                }
                            })
                        }
                        else{
                            items.push(quote!{
                                s.field(d+1, #field_string);
                                #field_name.ser_ron(d+1, s);
                                s.conl();
                            })
                        }
                        field_names.push(field_name);
                    }
                }
                match_item.push(quote!{
                    Self::#ident {#(#field_names,) *} => {
                        s.out.push_str(#lit);
                        s.st_pre();
                        #(
                            #items
                        )*
                        s.st_post(d);
                    }
                });
            },
            Fields::Unnamed(fields_unnamed) => {
                let mut field_names = Vec::new();
                let mut str_names = Vec::new();
                let last = fields_unnamed.unnamed.len() - 1;
                for (index, field) in fields_unnamed.unnamed.iter().enumerate() {
                    let field_name = Ident::new(&format!("f{}", index), field.span());
                    if index != last{
                        str_names.push(quote!{
                            #field_name.ser_ron(d, s); s.out.push_str(", ");
                        });
                    }
                    else{
                        str_names.push(quote!{
                            #field_name.ser_ron(d, s);
                        });
                    }
                    field_names.push(field_name);
                }
                match_item.push(quote!{
                    Self::#ident (#(#field_names,) *) => {
                        s.out.push_str(#lit);
                        s.out.push('(');
                        #(#str_names) *
                        s.out.push(')');
                    }
                });
            },
        }
    }
    
    quote! {
        impl #impl_generics SerRon for #ident #ty_generics #bounded_where_clause {
            fn ser_ron(&self, d: usize, s: &mut makepad_tinyserde::SerRonState) {
                match self {
                    #(
                        #match_item
                    ) *
                }
            }
        }
    }
}


pub fn derive_de_ron_enum(input: &DeriveInput, enumeration: &DataEnum) -> TokenStream {
    
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let ident = &input.ident;
    let bound = parse_quote!(DeRon);
    let bounded_where_clause = where_clause_with_bound(&input.generics, bound);
    
    let mut match_item = Vec::new();
    
    for variant in &enumeration.variants {
        let ident = &variant.ident;
        let lit = LitStr::new(&ident.to_string(), ident.span());
        match &variant.fields {
            Fields::Unit => {
                match_item.push(quote!{
                    #lit => Self::#ident,
                })
            },
            Fields::Named(fields_named) => {
                let body = derive_de_ron_named(quote!{Self::#ident}, fields_named);
                match_item.push(quote!{
                    #lit => {#body},
                });
            },
            Fields::Unnamed(fields_unnamed) => {
                let mut field_names = Vec::new();
                for _ in &fields_unnamed.unnamed {
                    field_names.push(quote! {{let r = DeRon::de_ron(s,i)?;s.eat_comma_paren(i)?;r}});
                }
                match_item.push(quote!{
                    #lit => {s.paren_open(i)?;let r = Self::#ident(#(#field_names,) *); s.paren_close(i)?;r},
                });
            },
        }
    }
    
    quote! {
        impl #impl_generics DeRon for #ident #ty_generics #bounded_where_clause {
            fn de_ron(s: &mut DeRonState, i: &mut std::str::Chars) -> std::result::Result<Self,DeRonErr> {
                // we are expecting an identifier
                s.ident(i)?;
                std::result::Result::Ok(match s.identbuf.as_ref() {
                    #(
                        #match_item
                    ) *
                    _ => return std::result::Result::Err(s.err_enum(&s.identbuf))
                })
            }
        }
    }
}

pub fn derive_ser_ron_struct_unnamed(input: &DeriveInput, fields:&FieldsUnnamed) -> TokenStream {
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let bound = parse_quote!(SerRon);
    let bounded_where_clause = where_clause_with_bound(&input.generics, bound);
    let ident = &input.ident;

    let mut str_names = Vec::new();
    let last = fields.unnamed.len() - 1;
    for (index, field) in fields.unnamed.iter().enumerate() {
        let field_name = LitInt::new(&format!("{}", index), field.span());
        if index != last{
            str_names.push(quote!{
                self.#field_name.ser_ron(d, s);
                s.out.push_str(", ");
            })
        }
        else{
            str_names.push(quote!{
                self.#field_name.ser_ron(d, s);
            })
        }
    }
    quote! {
        impl #impl_generics SerRon for #ident #ty_generics #bounded_where_clause {
            fn ser_ron(&self, d: usize, s: &mut makepad_tinyserde::SerRonState) {
                s.out.push('(');
                #(
                    #str_names
                ) *
                s.out.push(')');
            }
        }
    }
}


pub fn derive_de_ron_struct_unnamed(input: &DeriveInput, fields:&FieldsUnnamed) -> TokenStream {
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let ident = &input.ident;
    let bound = parse_quote!(DeRon);
    let bounded_where_clause = where_clause_with_bound(&input.generics, bound);

    let mut items = Vec::new();
    for _ in &fields.unnamed {
        items.push(quote!{{let r = DeRon::de_ron(s,i)?;s.eat_comma_paren(i)?;r},});
    }

    quote! {
        impl #impl_generics DeRon for #ident #ty_generics #bounded_where_clause {
            fn de_ron(s: &mut makepad_tinyserde::DeRonState, i: &mut std::str::Chars) -> std::result::Result<Self,DeRonErr> {
                s.paren_open(i)?;
                let r = Self(
                    #(
                        #items
                    ) *
                );
                s.paren_close(i)?;
                std::result::Result::Ok(r)
            }
        }
    }
}