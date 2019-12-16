extern crate proc_macro;
extern crate proc_macro2;
use proc_macro2::{Span};

mod shared;
use crate::shared::*;

mod serde_bin;
use crate::serde_bin::*;

mod serde_ron;
use crate::serde_ron::*;

mod serde_json;
use crate::serde_json::*;

use syn::{
    parse_macro_input,
    Data,
    DataStruct,
    Fields,
    DeriveInput,
};

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

#[proc_macro_derive(SerRon)]
pub fn derive_ser_ron(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    // ok we have an ident, its either a struct or a enum
    let ts = match &input.data {
        Data::Struct(DataStruct {fields: Fields::Named(fields), ..}) => {
            derive_ser_ron_struct(&input, fields)
        },
        Data::Struct(DataStruct {fields: Fields::Unnamed(fields), ..}) => {
            derive_ser_ron_struct_unnamed(&input, fields)
        },
        Data::Enum(enumeration) => {
            derive_ser_ron_enum(&input, enumeration)
        },
        _ => error(Span::call_site(), "only structs or enums supported")
    };
    proc_macro::TokenStream::from(ts)
}

#[proc_macro_derive(DeRon)]
pub fn derive_de_ron(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    // ok we have an ident, its either a struct or a enum
    let ts = match &input.data {
        Data::Struct(DataStruct {fields: Fields::Named(fields), ..}) => {
            derive_de_ron_struct(&input, fields)
        },
        Data::Struct(DataStruct {fields: Fields::Unnamed(fields), ..}) => {
            derive_de_ron_struct_unnamed(&input, fields)
        },
        Data::Enum(enumeration) => {
            derive_de_ron_enum(&input, enumeration)
        },
        _ => error(Span::call_site(), "only structs or enums supported")
    };
    //println!("{}", ts.to_string());
    proc_macro::TokenStream::from(ts)
}

#[proc_macro_derive(SerJson)]
pub fn derive_ser_json(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    // ok we have an ident, its either a struct or a enum
    let ts = match &input.data {
        Data::Struct(DataStruct {fields: Fields::Named(fields), ..}) => {
            derive_ser_json_struct(&input, fields)
        },
        Data::Struct(DataStruct {fields: Fields::Unnamed(fields), ..}) => {
            derive_ser_json_struct_unnamed(&input, fields)
        },
        Data::Enum(enumeration) => {
            derive_ser_json_enum(&input, enumeration)
        },
        _ => error(Span::call_site(), "only structs or enums supported")
    };
    proc_macro::TokenStream::from(ts)
}

#[proc_macro_derive(DeJson)]
pub fn derive_de_json(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    // ok we have an ident, its either a struct or a enum
    let ts = match &input.data {
        Data::Struct(DataStruct {fields: Fields::Named(fields), ..}) => {
            derive_de_json_struct(&input, fields)
        },
        Data::Struct(DataStruct {fields: Fields::Unnamed(fields), ..}) => {
            derive_de_json_struct_unnamed(&input, fields)
        },
        Data::Enum(enumeration) => {
            derive_de_json_enum(&input, enumeration)
        },
        _ => error(Span::call_site(), "only structs or enums supported")
    };
    //println!("{}",ts.to_string());
    proc_macro::TokenStream::from(ts)
}
