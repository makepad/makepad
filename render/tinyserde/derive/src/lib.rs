extern crate proc_macro;
extern crate proc_macro2;

use proc_macro2::{TokenStream, Span};
use syn::{
    parse_macro_input,
    parse_quote,
    Ident,
    DeriveInput,
    Data,
    DataStruct,
    Fields,
    FieldsNamed,
    FieldsUnnamed,
    DataEnum,
    LitInt,
    Generics,
    WherePredicate,
    WhereClause
};
use quote::quote;
use quote::quote_spanned;
use syn::spanned::Spanned;

fn where_clause_with_bound(generics: &Generics, bound: TokenStream) -> WhereClause {
    let new_predicates = generics.type_params().map::<WherePredicate, _>( | param | {
        let param = &param.ident;
        parse_quote!(#param: #bound)
    });
    
    let mut generics = generics.clone();
    generics
        .make_where_clause()
        .predicates
        .extend(new_predicates);
    generics.where_clause.unwrap()
}

fn error(span: Span, msg: &str) -> TokenStream {
    let fmsg = format!("tinyserde_derive: {}", msg);
    quote_spanned!(span => compile_error!(#fmsg);)
}

fn derive_ser_bin_struct(input: &DeriveInput, fields: &FieldsNamed) -> TokenStream {
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let bound = parse_quote!(SerBin);
    let bounded_where_clause = where_clause_with_bound(&input.generics, bound);
    let ident = &input.ident;
    let fieldname = fields.named.iter().map( | f | f.ident.clone().unwrap()).collect::<Vec<_>>();
    
    quote! {
        impl #impl_generics SerBin for #ident #ty_generics #bounded_where_clause {
            fn ser_bin(&self, s: &mut SerBinData) {
                #(
                    self.#fieldname.ser_bin(s);
                ) *
            }
        }
    }
}

fn derive_ser_bin_struct_unnamed(input: &DeriveInput, fields:&FieldsUnnamed) -> TokenStream {
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let bound = parse_quote!(SerBin);
    let bounded_where_clause = where_clause_with_bound(&input.generics, bound);
    let ident = &input.ident;

    let mut fieldname = Vec::new();
    for (index, field) in fields.unnamed.iter().enumerate() {
        fieldname.push(LitInt::new(&format!("{}", index), field.span()));
    }
    quote! {
        impl #impl_generics SerBin for #ident #ty_generics #bounded_where_clause {
            fn ser_bin(&self, s: &mut SerBinData) {
                #(
                    self.#fieldname.ser_bin(s);
                ) *
            }
        }
    }
}

fn derive_de_bin_struct(input: &DeriveInput, fields:&FieldsNamed) -> TokenStream {
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let ident = &input.ident;
    let bound = parse_quote!(DeBin);
    let bounded_where_clause = where_clause_with_bound(&input.generics, bound);
    let fieldname = fields.named.iter().map( | f | f.ident.clone().unwrap()).collect::<Vec<_>>();

    quote! {
        impl #impl_generics DeBin for #ident #ty_generics #bounded_where_clause {
            fn de_bin(d: &mut DeBinData) -> Self {
                Self {
                    #(
                        #fieldname: DeBin::de_bin(d),
                    ) *
                }
            }
        }
    }
}

fn derive_de_bin_struct_unnamed(input: &DeriveInput, fields:&FieldsUnnamed) -> TokenStream {
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let ident = &input.ident;
    let bound = parse_quote!(DeBin);
    let bounded_where_clause = where_clause_with_bound(&input.generics, bound);

    let mut fieldname = Vec::new();
    for (index, field) in fields.unnamed.iter().enumerate() {
        fieldname.push(LitInt::new(&format!("{}", index), field.span()));
    }

    quote! {
        impl #impl_generics DeBin for #ident #ty_generics #bounded_where_clause {
            fn de_bin(d: &mut DeBinData) -> Self {
                Self {
                    #(
                        #fieldname: DeBin::de_bin(d),
                    ) *
                }
            }
        }
    }
}

fn derive_ser_bin_enum(input: &DeriveInput, enumeration: &DataEnum) -> TokenStream {
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let bound = parse_quote!(SerBin);
    let bounded_where_clause = where_clause_with_bound(&input.generics, bound);
    
    let ident = &input.ident;
    
    let mut match_item = Vec::new();
    
    for (index, variant) in enumeration.variants.iter().enumerate() {
        let lit = LitInt::new(&format!("{}u16", index), ident.span());
        let ident = &variant.ident;
        match &variant.fields {
            Fields::Unit => {
                match_item.push(quote!{
                    Self::#ident => #lit.ser_bin(s),
                })
            },
            Fields::Named(fields_named) => {
                let mut field_names = Vec::new();
                for field in &fields_named.named {
                    if let Some(ident) = &field.ident {
                        field_names.push(ident);
                    }
                }
                match_item.push(quote!{
                    Self::#ident {#(#field_names,) *} => {
                        #lit.ser_bin(s);
                        #(#field_names.ser_bin(s);) *
                    }
                });
            },
            Fields::Unnamed(fields_unnamed) => {
                let mut field_names = Vec::new();
                for (index, field) in fields_unnamed.unnamed.iter().enumerate() {
                    field_names.push(Ident::new(&format!("f{}", index), field.span()));
                }
                match_item.push(quote!{
                    Self::#ident (#(#field_names,) *) => {
                        #lit.ser_bin(s);
                        #(#field_names.ser_bin(s);) *
                    }
                });
            },
        }
    }
    
    quote! {
        impl #impl_generics SerBin for #ident #ty_generics #bounded_where_clause {
            fn ser_bin(&self, s: &mut SerBinData) {
                match self {
                    #(
                        #match_item
                    ) *
                }
            }
        }
    }
}


fn derive_de_bin_enum(input: &DeriveInput, enumeration: &DataEnum) -> TokenStream {
    
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let ident = &input.ident;
    let bound = parse_quote!(DeBin);
    let bounded_where_clause = where_clause_with_bound(&input.generics, bound);
    
    let mut match_item = Vec::new();
    
    for (index, variant) in enumeration.variants.iter().enumerate() {
        let lit = LitInt::new(&format!("{}u16", index), ident.span());
        let ident = &variant.ident;
        match &variant.fields {
            Fields::Unit => {
                match_item.push(quote!{
                    #lit => Self::#ident,
                })
            },
            Fields::Named(fields_named) => {
                let mut field_names = Vec::new();
                for field in &fields_named.named {
                    if let Some(ident) = &field.ident {
                        field_names.push(quote!{#ident: DeBin::de_bin(d)});
                    }
                }
                match_item.push(quote!{
                    #lit => Self::#ident {#(#field_names,) *},
                });
            },
            Fields::Unnamed(fields_unnamed) => {
                let mut field_names = Vec::new();
                for _ in &fields_unnamed.unnamed {
                    field_names.push(quote! {DeBin::de_bin(d)});
                }
                match_item.push(quote!{
                    #lit => Self::#ident(#(#field_names,) *),
                });
            },
        }
    }
    
    quote! {
        impl #impl_generics DeBin for #ident #ty_generics #bounded_where_clause {
            fn de_bin(d: &mut DeBinData) -> Self {
                let id: u16 = DeBin::de_bin(d);
                match id {
                    #(
                        #match_item
                    ) *
                    _ => panic!("enum match failed")
                }
            }
        }
    }
}

#[proc_macro_derive(SerBin)]
pub fn derive_ser_bin(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    // ok we have an ident, its either a struct or a enum
    let ts = match &input.data {
        Data::Struct(DataStruct {fields: Fields::Named(fields), ..}) => {
            derive_ser_bin_struct(&input, fields)
        },
        Data::Struct(DataStruct {fields: Fields::Unnamed(fields), ..}) => {
            derive_ser_bin_struct_unnamed(&input, fields)
        },
        Data::Enum(enumeration) => {
            derive_ser_bin_enum(&input, enumeration)
        },
        _ => error(Span::call_site(), "only structs or enums supported")
    };
    proc_macro::TokenStream::from(ts)
}


#[proc_macro_derive(DeBin)]
pub fn derive_de_bin(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    // ok we have an ident, its either a struct or a enum
    let ts = match &input.data {
        Data::Struct(DataStruct {fields: Fields::Named(fields), ..}) => {
            derive_de_bin_struct(&input, fields)
        },
        Data::Struct(DataStruct {fields: Fields::Unnamed(fields), ..}) => {
            derive_de_bin_struct_unnamed(&input, fields)
        },
        Data::Enum(enumeration) => {
            derive_de_bin_enum(&input, enumeration)
        },
        _ => error(Span::call_site(), "only structs or enums supported")
    };
    
    proc_macro::TokenStream::from(ts)
}
