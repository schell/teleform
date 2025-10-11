//! Provides derive macros for `teleform`.
use std::collections::HashSet;

use quote::quote;
use syn::{Data, DataStruct, DeriveInput, Fields, FieldsNamed, Index, TypeTuple};

struct Composite {
    function_body: proc_macro2::TokenStream,
    where_constraints: Vec<proc_macro2::TokenStream>,
}

fn get_composite(input: &DeriveInput) -> syn::Result<Composite> {
    let name = &input.ident;
    let fields = match &input.data {
        Data::Struct(DataStruct {
            fields: Fields::Named(FieldsNamed { named, .. }),
            ..
        }) => named,
        _ => {
            return Err(syn::Error::new(
                name.span(),
                "deriving TeleSync only supports structs with named fields".to_string(),
            ));
        }
    };

    let where_constraints: Vec<_> = fields
        .iter()
        .map(|field| &field.ty)
        .collect::<HashSet<_>>()
        .into_iter()
        .map(|ty| {
            quote! {
                #ty: tele::HasDependencies
            }
        })
        .collect();
    let composites: Vec<_> = fields
        .iter()
        .map(|field| {
            // UNWRAP: safe because we only support structs (which all have named fields)
            let ident = field.ident.clone().unwrap();
            quote! {
                .merge(self.#ident.dependencies())
            }
        })
        .collect();
    let function_body = quote! {
        tele::Dependencies::default()
            #(#composites)*
    };
    Ok(Composite {
        where_constraints,
        function_body,
    })
}

#[proc_macro_derive(HasDependencies)]
pub fn derive_has_dependencies(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input: DeriveInput = syn::parse_macro_input!(input);
    let name = &input.ident;

    let Composite {
        function_body: composite,
        where_constraints,
    } = match get_composite(&input) {
        Ok(c) => c,
        Err(e) => return e.into_compile_error().into(),
    };
    let output = quote! {
        impl tele::HasDependencies for #name
        where
            #(#where_constraints),*
        {
            fn dependencies(&self) -> tele::Dependencies {
                #composite
            }
        }
    };
    output.into()
}

#[proc_macro]
pub fn impl_has_dependencies_tuples(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let tuple: TypeTuple = syn::parse_macro_input!(input);
    let tys = tuple.elems.iter().collect::<Vec<_>>();
    let deps = tys
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let ndx = Index::from(i);
            quote! {
                .merge(self.#ndx.dependencies())
            }
        })
        .collect::<Vec<_>>();
    let output = quote! {
        impl<#(#tys),*> tele::HasDependencies for #tuple
        where
            #(#tys: tele::HasDependencies),*,
        {
            fn dependencies(&self) -> tele::Dependencies {
                tele::Dependencies::default()
                    #(#deps)*

            }
        }
    };
    output.into()
}
