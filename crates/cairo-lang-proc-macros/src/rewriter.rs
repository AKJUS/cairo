//! Procedural macros.
use proc_macro::TokenStream;
use quote::__private::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{DeriveInput, Generics, parse_macro_input, parse_quote};

/// Derives a SemanticObject implementation for structs and enums.
pub fn derive_semantic_object(input: TokenStream) -> TokenStream {
    // Parse the input tokens into a syntax tree.
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let generics = &input.generics;

    // TODO(yuval/shahar): extract the lifetime here and use it instead of `'a` below.

    let body = match &input.data {
        syn::Data::Struct(structure) => emit_struct_semantic_object(name, generics, structure),
        syn::Data::Enum(enm) => emit_enum_semantic_object(name, generics, enm),
        syn::Data::Union(_) => panic!("Unions are not supported"),
    };

    quote! {
        #[allow(unused_parens)]
        #body
    }
    .into()
}

/// Emits a SemanticObject implementation for a struct.
fn emit_struct_semantic_object(
    name: &syn::Ident,
    generics: &Generics,
    structure: &syn::DataStruct,
) -> TokenStream2 {
    let (deps_vec, pattern, field_rewrites) = emit_fields_semantic_object(&structure.fields);

    // Create impl generics by cloning original generics and adding T and Error parameters
    let mut impl_generics = generics.clone();

    // Create the T parameter with all the dependencies
    let t_param = if deps_vec.is_empty() {
        parse_quote! { T }
    } else {
        // Build the trait bounds manually
        let mut bounds = quote! {};
        for (i, dep) in deps_vec.iter().enumerate() {
            if i == 0 {
                bounds = quote! { #dep };
            } else {
                bounds = quote! { #bounds + #dep };
            }
        }
        parse_quote! { T: #bounds }
    };
    impl_generics.params.push(t_param);
    impl_generics.params.push(parse_quote! { Error });

    let (impl_g, _, where_g) = impl_generics.split_for_impl();
    let (_, ty_g, _) = generics.split_for_impl();

    let crt = semantic_crate();

    quote! {
        impl #impl_g #crt::SemanticObject<T, Error> for #name #ty_g #where_g {
            fn default_rewrite(&mut self, rewriter: &mut T) -> Result<#crt::substitution::RewriteResult, Error> {
                let Self #pattern = self;
                let mut result = #crt::substitution::RewriteResult::NoChange;
                #field_rewrites

                Ok(result)
            }
        }
    }
}

/// Emits a SemanticObject implementation for an enum.
fn emit_enum_semantic_object(
    name: &syn::Ident,
    generics: &Generics,
    enm: &syn::DataEnum,
) -> TokenStream2 {
    let mut variant_rewrites = quote! {};
    let mut all_deps_vec = Vec::new();

    for variant in enm.variants.iter() {
        let variant_name = &variant.ident;

        let (current_deps_vec, pattern, field_rewrites) =
            emit_fields_semantic_object(&variant.fields);

        // Add all dependencies from this variant to our collection
        all_deps_vec.extend(current_deps_vec);

        variant_rewrites = quote! {
            #variant_rewrites
            #name :: #variant_name #pattern => {
                #field_rewrites
            }
        }
    }

    // Create impl generics by cloning original generics and adding T and Error parameters
    let mut impl_generics = generics.clone();

    // Create the T parameter with all the dependencies
    let t_param = if all_deps_vec.is_empty() {
        parse_quote! { T }
    } else {
        // Build the trait bounds manually
        let mut bounds = quote! {};
        for (i, dep) in all_deps_vec.iter().enumerate() {
            if i == 0 {
                bounds = quote! { #dep };
            } else {
                bounds = quote! { #bounds + #dep };
            }
        }
        parse_quote! { T: #bounds }
    };
    impl_generics.params.push(t_param);
    impl_generics.params.push(parse_quote! { Error });

    let (impl_g, _, where_g) = impl_generics.split_for_impl();
    let (_, ty_g, _) = generics.split_for_impl();

    let crt = semantic_crate();

    quote! {
        impl #impl_g #crt::SemanticObject<T, Error> for #name #ty_g #where_g {
            fn default_rewrite(&mut self, rewriter: &mut T) -> Result<#crt::substitution::RewriteResult, Error> {
                let mut result = #crt::substitution::RewriteResult::NoChange;
                match self {
                    #variant_rewrites
                }
                Ok(result)
            }
        }
    }
}

/// Helper function for emit_struct_semantic_object and emit_enum_semantic_object. Both struct, and
/// a variant use a type called [syn::Fields].
/// This function builds and returns an unpacking pattern and code for `fmt()` on these fields.
fn emit_fields_semantic_object(
    fields: &syn::Fields,
) -> (Vec<TokenStream2>, TokenStream2, TokenStream2) {
    let mut pattern = quote! {};
    let mut field_rewrites = quote! {};
    let mut deps_vec = Vec::new();

    for (i, field) in fields.iter().enumerate() {
        let field_ident = field
            .ident
            .clone()
            .unwrap_or_else(|| syn::Ident::new(&format!("v{i}"), Span::call_site()));

        let has_dont_rewrite_attr = field
            .attrs
            .iter()
            .any(|a| a.path().segments.len() == 1 && a.path().segments[0].ident == "dont_rewrite");

        if has_dont_rewrite_attr {
            if let Some(field_ident) = &field.ident {
                pattern = quote! { #pattern #field_ident: _, };
            } else {
                pattern = quote! { #pattern _, };
            }
        } else {
            pattern = quote! { #pattern #field_ident, };
            let rewrite_expr = emit_expr_for_ty(&mut deps_vec, quote! {#field_ident}, &field.ty);
            field_rewrites = quote! {
                #field_rewrites
                #rewrite_expr;
            }
        };
    }

    match fields {
        syn::Fields::Named(_) => {
            pattern = quote! { { #pattern } };
        }
        syn::Fields::Unnamed(_) => {
            pattern = quote! { ( #pattern ) };
        }
        syn::Fields::Unit => {
            pattern = quote! {};
        }
    };
    (deps_vec, pattern, field_rewrites)
}

fn emit_expr_for_ty(
    deps_vec: &mut Vec<TokenStream2>,
    item: TokenStream2,
    ty: &syn::Type,
) -> TokenStream2 {
    let crt = semantic_crate();

    deps_vec.push(quote! { #crt::substitution::SemanticRewriter<#ty, Error> });
    quote! {
        match #crt::substitution::SemanticRewriter::<#ty, Error>::internal_rewrite(rewriter, #item)? {
            #crt::substitution::RewriteResult::Modified => {
                result = #crt::substitution::RewriteResult::Modified;
            }
            #crt::substitution::RewriteResult::NoChange => {}
        }
    }
}

/// Returns the identifier of the semantic crate. This is needed, since inside the semantic
/// crate, it needs to be referred to as `crate` and no `semantic`.
fn semantic_crate() -> syn::Ident {
    let crate_name = std::env::var("CARGO_PKG_NAME").unwrap();
    let res = match crate_name.as_str() {
        "cairo-lang-semantic" | "cairo_lang_semantic" => "crate",
        _ => "cairo_lang_semantic",
    };
    syn::Ident::new(res, Span::call_site())
}
