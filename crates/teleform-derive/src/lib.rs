//! Provides derive macros for `tele::TeleSync`.
use std::collections::HashSet;

use quote::quote;
use syn::{Attribute, Data, DataStruct, DeriveInput, Fields, FieldsNamed};

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
        .map(|ty| quote! {
            #ty: tele::TeleEither
        })
        .collect();
    let composites: Vec<_> = fields
        .iter()
        .map(|field| {
            // UNWRAP: safe because we only support structs (which all have named fields)
            let ident = field.ident.clone().unwrap();
            let ty = &field.ty;
            quote! {
                #ident: <#ty as tele::TeleEither>::either(self.#ident, other.#ident),
            }
        })
        .collect();
    let function_body = quote! {
        #name {
            #(#composites)*
        }
    };
    Ok(Composite {
        where_constraints,
        function_body,
    })
}

fn get_should_recreate_update(
    ast: &Data,
) -> syn::Result<(proc_macro2::TokenStream, proc_macro2::TokenStream)> {
    let fields = match *ast {
        Data::Struct(DataStruct {
            fields: Fields::Named(FieldsNamed { named: ref x, .. }),
            ..
        }) => x,
        _ => {
            return Ok((
                quote! { compile_error!("deriving TeleSync only supports structs with named fields")},
                quote! {},
            ))
        }
    };

    let mut update_idents = vec![];
    let mut recreate_idents = vec![];
    'outer: for field in fields.into_iter() {
        // UNWRAP: safe because we only support structs (which all have named fields)
        let ident = field.ident.clone().unwrap();
        for att in field.attrs.iter() {
            let mut ignore_should_update = false;
            let mut should_recreate = false;
            if att.path().is_ident("tele") {
                att.parse_nested_meta(|meta| {
                    if meta.path.is_ident("ignore") {
                        ignore_should_update = true;
                        Ok(())
                    } else if meta.path.is_ident("should_recreate") {
                        should_recreate = true;
                        Ok(())
                    } else {
                        Err(meta.error(format!(
                            "unsupported field attribute {:?} - must be one of \
                             'ignore' or 'should_recreate'",
                            meta.path
                                .get_ident()
                                .map(|id| id.to_string())
                                .unwrap_or("unknown".to_string())
                        )))
                    }
                })?;
            }
            if ignore_should_update {
                continue 'outer;
            }
            if should_recreate {
                recreate_idents.push(ident);
                continue 'outer;
            }
        }
        update_idents.push(ident);
    }

    Ok((
        quote! {
            #(self.#recreate_idents != other.#recreate_idents ||)* false
        },
        quote! {
            #(self.#update_idents != other.#update_idents ||)* false
        },
    ))
}

#[derive(Debug, Default)]
struct ImplDetails {
    helper: Option<syn::TypeReference>,
    create: Option<syn::Ident>,
    update: Option<syn::Ident>,
    delete: Option<syn::Ident>,
}

fn get_impl_details(attrs: &[Attribute]) -> syn::Result<ImplDetails> {
    let mut details = ImplDetails::default();
    for att in attrs.iter() {
        if att.path().is_ident("tele") {
            att.parse_nested_meta(|meta| {
                if meta.path.is_ident("helper") {
                    let value = meta.value()?;
                    let tyref: syn::TypeReference = value.parse()?;
                    details.helper = Some(tyref);
                } else if meta.path.is_ident("create") {
                    let value = meta.value()?;
                    let ident: syn::Ident = value.parse()?;
                    details.create = Some(ident);
                } else if meta.path.is_ident("update") {
                    let value = meta.value()?;
                    let ident: syn::Ident = value.parse()?;
                    details.update = Some(ident);
                } else if meta.path.is_ident("delete") {
                    let value = meta.value()?;
                    let ident: syn::Ident = value.parse()?;
                    details.delete = Some(ident);
                } else {
                    return Err(meta.error(format!(
                        "unknown attribute {:?} - must be one of 'helper', \
                         'create', 'update' or 'delete'",
                        meta.path
                            .get_ident()
                            .map(|id| id.to_string())
                            .unwrap_or("unknown".to_string())
                    )));
                }
                Ok(())
            })?;
        }
    }
    Ok(details)
}

#[proc_macro_derive(TeleSync, attributes(tele))]
pub fn derive_telesync(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input: DeriveInput = syn::parse_macro_input!(input);
    let name = &input.ident;

    let details = match get_impl_details(&input.attrs) {
        Ok(d) => d,
        Err(e) => return e.into_compile_error().into(),
    };
    let helper = details.helper.unwrap_or(syn::parse_quote! {&'a ()});
    let create = details
        .create
        .map(|create| {
            quote! {
                #create(self, apply, helper, name)
            }
        })
        .unwrap_or_else(|| quote! { compile_error!("missing tele_create_with attribute")});
    let update = details
        .update
        .map(|update| {
            quote! {
                #update(self, apply, helper, name, previous)
            }
        })
        .unwrap_or_else(|| quote! {compile_error!("missing tele_update_with attribute")});
    let delete = details
        .delete
        .map(|delete| {
            quote! {
                #delete(self, apply, helper, name)
            }
        })
        .unwrap_or_else(|| quote! {compile_error!("missing tele_delete_with attribute")});
    let Composite { function_body: composite, where_constraints } = match get_composite(&input) {
        Ok(c) => c,
        Err(e) => return e.into_compile_error().into(),
    };
    let (should_recreate, should_update) = match get_should_recreate_update(&input.data) {
        Ok(x) => x,
        Err(e) => return e.into_compile_error().into(),
    };

    let output = quote! {
        impl tele::TeleSync for #name
        where
            #(#where_constraints),*
        {
            type Provider<'a> = #helper;

            fn composite(self, other: Self) -> Self {
                #composite
            }

            fn should_recreate(&self, other: &Self) -> bool {
                #should_recreate
            }

            fn should_update(&self, other: &Self) -> bool {
                #should_update
            }

            fn create<'ctx, 'a>(
                &'a mut self,
                apply: bool,
                helper: Self::Provider<'ctx>,
                name: &'a str,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + 'a>>
            where
                'ctx: 'a,
            {
                Box::pin(#create)
            }

            fn update<'ctx, 'a>(
                &'a mut self,
                apply: bool,
                helper: Self::Provider<'ctx>,
                name: &'a str,
                previous: &'a Self,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + 'a>>
            where
                'ctx: 'a,
            {
                Box::pin(#update)
            }

            fn delete<'ctx, 'a>(
                &'a self,
                apply: bool,
                helper: Self::Provider<'ctx>,
                name: &'a str,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + 'a>>
            where
                'ctx: 'a,
            {
                Box::pin(#delete)
            }
        }
    };
    output.into()
}
