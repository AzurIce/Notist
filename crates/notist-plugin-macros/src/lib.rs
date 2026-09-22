//! Attribute expansion for the public API re-exported by notist-plugin-sdk.
use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::{format_ident, quote};
use std::collections::{BTreeMap, BTreeSet};
use syn::{
    Expr, FnArg, Ident, ItemFn, Pat, Path, ReturnType, Token,
    ext::IdentExt,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    spanned::Spanned,
};

fn sdk_path() -> syn::Result<Tokens> {
    match proc_macro_crate::crate_name("notist-plugin-sdk") {
        Ok(proc_macro_crate::FoundCrate::Itself) => Ok(quote!(::notist_plugin_sdk)),
        Ok(proc_macro_crate::FoundCrate::Name(name)) => {
            let name = format_ident!("{name}");
            Ok(quote!(::#name))
        }
        Err(error) => Err(syn::Error::new(proc_macro2::Span::call_site(), error)),
    }
}

struct DefaultArgument {
    name: Ident,
    value: Expr,
}

impl Parse for DefaultArgument {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name = input.parse()?;
        input.parse::<Token![=]>()?;
        Ok(Self {
            name,
            value: input.parse()?,
        })
    }
}

#[derive(Default)]
struct Options {
    defaults: BTreeMap<String, DefaultArgument>,
}

impl Parse for Options {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut options = Self::default();
        if input.is_empty() {
            return Ok(options);
        }
        let keyword: Ident = input.parse()?;
        if keyword != "defaults" {
            return Err(syn::Error::new(
                keyword.span(),
                "expected defaults(parameter = expression, ...)",
            ));
        }
        let content;
        syn::parenthesized!(content in input);
        for default in content.parse_terminated(DefaultArgument::parse, Token![,])? {
            let name = default.name.unraw().to_string();
            if options.defaults.contains_key(&name) {
                return Err(syn::Error::new(
                    default.name.span(),
                    "duplicate parameter default",
                ));
            }
            options.defaults.insert(name, default);
        }
        if !input.is_empty() {
            input.parse::<Token![,]>()?;
        }
        Ok(options)
    }
}

#[proc_macro_attribute]
pub fn func(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let options = parse_macro_input!(attributes as Options);
    let function = parse_macro_input!(item as ItemFn);
    sdk_path()
        .and_then(|sdk| expand_func(options, function, &sdk))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_func(mut options: Options, function: ItemFn, sdk: &Tokens) -> syn::Result<Tokens> {
    let signature = &function.sig;
    if signature.asyncness.is_some()
        || signature.unsafety.is_some()
        || signature.abi.is_some()
        || signature.variadic.is_some()
        || !signature.generics.params.is_empty()
        || signature.generics.where_clause.is_some()
    {
        return Err(syn::Error::new(
            signature.span(),
            "Notist exports must be synchronous, safe, non-generic Rust functions",
        ));
    }
    let name = &signature.ident;
    let export = name.unraw().to_string();
    if export.starts_with("__notist_")
        || matches!(
            export.as_str(),
            "alloc" | "notist_register" | "notist_registration" | "notist_dispatch"
        )
    {
        return Err(syn::Error::new(
            name.span(),
            "function name is reserved by the Notist plugin ABI",
        ));
    }
    let mut params = Vec::new();
    let mut conversions = Vec::new();
    for argument in &signature.inputs {
        let FnArg::Typed(argument) = argument else {
            return Err(syn::Error::new(
                argument.span(),
                "Notist exports must be free functions",
            ));
        };
        let Pat::Ident(binding) = &*argument.pat else {
            return Err(syn::Error::new(
                argument.pat.span(),
                "Notist parameters require identifier names",
            ));
        };
        if binding.by_ref.is_some() || binding.subpat.is_some() {
            return Err(syn::Error::new(
                binding.span(),
                "Notist parameters require identifier names",
            ));
        }
        let parameter = binding.ident.unraw().to_string();
        let ty = &argument.ty;
        let default = match options.defaults.remove(&parameter) {
            Some(default) => {
                let value = default.value;
                quote!(Some(<#ty as #sdk::PluginValue>::into_value(#value)))
            }
            None => quote!(None),
        };
        params.push(quote!(#sdk::abi::Parameter {
            name: #parameter.into(),
            ty: <#ty as #sdk::PluginValue>::ty(),
            default: #default,
        }));
        conversions.push(quote!(<#ty as #sdk::PluginValue>::from_value(
            arguments.next().ok_or_else(|| format!("missing argument `{}`", #parameter))?
        )?));
    }
    if let Some((_, default)) = options.defaults.first_key_value() {
        return Err(syn::Error::new(
            default.name.span(),
            "default names an unknown parameter",
        ));
    }
    let result = match &signature.output {
        ReturnType::Default => quote!(()),
        ReturnType::Type(_, ty) => quote!(#ty),
    };
    let arity = signature.inputs.len();
    let vis = &function.vis;
    let cfg: Vec<_> = function
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();
    let entry = format_ident!("__notist_export_{}", name.unraw());
    let descriptor = format_ident!("__notist_descriptor_{}", name.unraw());
    let invoke = format_ident!("__notist_invoke_{}", name.unraw());
    let wasm = format_ident!("__notist_wasm_{}", name.unraw());
    Ok(quote! {
        #function

        #(#cfg)*
        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        #vis const #entry: #sdk::__private::Export = #sdk::__private::Export {
            name: #export,
            descriptor: #descriptor,
            invoke: #invoke,
        };

        #(#cfg)*
        fn #descriptor() -> #sdk::abi::Function {
            #sdk::abi::Function {
                export: #export.into(),
                params: vec![#(#params),*],
                result: <#result as #sdk::PluginResult>::ty(),
            }
        }

        #(#cfg)*
        fn #invoke(input: &[u8]) -> Vec<u8> {
            let function = #name;
            #sdk::__private::dispatch(input, |arguments| {
                if arguments.len() > #arity { return Err("excess arguments".into()); }
                #[allow(unused_mut, unused_variables)]
                let mut arguments = arguments.into_iter();
                <#result as #sdk::PluginResult>::into_result(function(#(#conversions),*))
            })
        }

        #(#cfg)*
        #[cfg(target_arch = "wasm32")]
        #[unsafe(export_name = #export)]
        unsafe extern "C" fn #wasm(pointer: u32, length: u32) -> u64 {
            let input = unsafe { #sdk::__private::input(pointer, length) };
            #sdk::__private::output(#invoke(input))
        }
    })
}

#[proc_macro]
pub fn init_plugin(input: TokenStream) -> TokenStream {
    let Init { exports, elements } = parse_macro_input!(input as Init);
    sdk_path()
        .and_then(|sdk| expand_init(exports, elements, &sdk))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

struct Init {
    exports: Punctuated<Path, Token![,]>,
    elements: Option<Path>,
}
impl Parse for Init {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let elements = if input.peek(Ident) && input.peek2(Token![=]) {
            let name: Ident = input.parse()?;
            if name != "elements" {
                return Err(syn::Error::new(name.span(), "expected elements"));
            }
            input.parse::<Token![=]>()?;
            let path = input.parse()?;
            input.parse::<Token![;]>()?;
            Some(path)
        } else {
            None
        };
        Ok(Self {
            elements,
            exports: Punctuated::parse_terminated(input)?,
        })
    }
}

fn expand_init(
    mut exports: Punctuated<Path, Token![,]>,
    elements: Option<Path>,
    sdk: &Tokens,
) -> syn::Result<Tokens> {
    let mut names = BTreeSet::new();
    for export in &exports {
        if export.segments.iter().any(|s| !s.arguments.is_empty()) {
            return Err(syn::Error::new(
                export.span(),
                "expected a function path without generic arguments",
            ));
        }
        let name = export.segments.last().unwrap().ident.unraw().to_string();
        if !names.insert(name) {
            return Err(syn::Error::new(
                export.span(),
                "duplicate exported function name",
            ));
        }
    }
    for export in &mut exports {
        let segment = export.segments.last_mut().unwrap();
        segment.ident = format_ident!("__notist_export_{}", segment.ident.unraw());
    }
    let exports: Vec<_> = exports.iter().collect();
    let models = elements.map(|path| quote! { registration.elements = #path(); });
    Ok(quote! {
        pub fn notist_registration() -> #sdk::abi::Registration {
            #[allow(unused_mut)]
            let mut registration = #sdk::__private::registration(&[#(#exports),*]);
            #models
            registration
        }

        pub fn notist_dispatch(export: &str, input: &[u8]) -> Vec<u8> {
            #sdk::__private::invoke_export(&[#(#exports),*], export, input)
        }

        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn alloc(length: u32) -> u32 { #sdk::__private::alloc(length) }

        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn notist_register(_: u32, _: u32) -> u64 {
            #sdk::__private::output(#sdk::__private::registration_bytes(&notist_registration()))
        }
    })
}

#[cfg(test)]
mod tests;
