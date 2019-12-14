use proc_macro2::{TokenStream,Span};
use quote::quote_spanned;
use syn::{
    parse_quote,
    Generics,
    WherePredicate,
    WhereClause
};

pub fn where_clause_with_bound(generics: &Generics, bound: TokenStream) -> WhereClause {
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

pub fn error(span: Span, msg: &str) -> TokenStream {
    let fmsg = format!("tinyserde_derive: {}", msg);
    quote_spanned!(span => compile_error!(#fmsg);)
}
