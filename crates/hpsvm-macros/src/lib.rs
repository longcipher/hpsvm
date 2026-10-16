//! `#[svm_instruction]` for hpsvm-owned instructions.
//!
//! The macro declares a builtin instruction handler from a plain function,
//! removing hand-written entrypoint/registerer boilerplate. Scope is
//! strictly hpsvm-owned builtins and precompiles (AD-10): the generated code
//! produces an Agave `BuiltinFunctionRegisterer`-compatible entrypoint that
//! is installed via [`Svm::add_builtin`](https://github.com/longcipher/hpsvm),
//! and it must never be used for SBF program dispatch.
//!
//! # Contract
//!
//! ```ignore
//! use hpsvm::SvmInstructionCtx;
//! use hpsvm_macros::svm_instruction;
//! use solana_transaction::InstructionError;
//!
//! #[svm_instruction(
//!     program_id = "84LtecYhx229KabnaiMixiuD5Tey4bBo6N4yzZ81Vzpz",
//!     compute_units = 150
//! )]
//! fn move_seven(ctx: &mut SvmInstructionCtx) -> Result<(), InstructionError> {
//!     // ... mutate ctx.accounts / read ctx.data ...
//!     Ok(())
//! }
//!
//! // Generated: `pub struct MoveSeven;` with
//! // `MoveSeven::PROGRAM_ID`, `MoveSeven::COMPUTE_UNITS`,
//! // `MoveSeven::execute`, `MoveSeven::program_id`,
//! // `MoveSeven::register`, and `MoveSeven::registerer`.
//! ```
//!
//! The input function keeps its exact shape: only a free function with
//! precisely one argument `ctx: &mut SvmInstructionCtx` returning
//! `Result<(), InstructionError>` is accepted. Anything else is rejected
//! with `compile_error!`:
//!
//! ```compile_fail
//! use hpsvm_macros::svm_instruction;
//!
//! #[svm_instruction(
//!     program_id = "84LtecYhx229KabnaiMixiuD5Tey4bBo6N4yzZ81Vzpz",
//!     compute_units = 150
//! )]
//! fn not_a_handler() -> Result<(), ()> {
//!     Ok(())
//! }
//! ```
//!
//! ```compile_fail
//! use hpsvm_macros::svm_instruction;
//!
//! #[svm_instruction(program_id = "not-base58!!", compute_units = 150)]
//! fn bad_id(ctx: &mut SvmInstructionCtx) -> Result<(), InstructionError> {
//!     Ok(())
//! }
//! ```
//!
//! # Expansion
//!
//! For `fn move_seven(...)`, the macro emits the original function
//! unchanged (so it stays directly callable as the manually-implemented
//! equivalent) plus:
//!
//! - `pub struct MoveSeven;`: zero-sized marker type. The name is the `UpperCamelCase` of the
//!   function name.
//! - `MoveSeven::PROGRAM_ID`: the attribute id as a true `const` (`Address::new_from_array`),
//!   decoded from base58 at expansion time.
//! - `MoveSeven::COMPUTE_UNITS: u64`: the declared CU charge.
//! - `MoveSeven::execute`: `#[inline]` forwarder to the free function.
//! - `MoveSevenEntrypoint`: agave builtin entrypoint with the exact
//!   `BuiltinFunction` signature. It consumes `COMPUTE_UNITS` through the
//!   same `compute_meter.consume_checked` call Agave's own
//!   `declare_process_instruction!` uses, snapshots the running instruction
//!   into an owned [`SvmInstructionCtx`](https://github.com/longcipher/hpsvm),
//!   runs `execute`, writes account mutations back, and maps
//!   `Ok(()) -> Ok(0)` / `Err(InstructionError) -> Err(boxed)` so the
//!   runtime downcasts to the original `InstructionError`.
//! - `MoveSeven::registerer`: loader-level `BuiltinFunctionRegisterer` that installs the entrypoint
//!   into a `BuiltinProgram`, mirroring what `solana_system_program` does for its own
//!   `process_instruction`.
//! - `MoveSeven::register`: `svm.add_builtin(program_id, registerer)` helper, generic over the VM's
//!   type family.
//!
//! All generated paths are absolute (`::hpsvm::…`,
//! `::solana_address::…`, `::solana_program_runtime::…`) so the expansion
//! survives any module layout. The downstream crate must have those crates
//! available (as `hpsvm`'s own test suite does through its dependencies).

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{
    Expr, FnArg, GenericArgument, ItemFn, Lit, Meta, PathArguments, ReturnType, Safety, Type,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    token::Comma,
};

/// Parsed `#[svm_instruction(...)]` arguments.
#[derive(Debug, PartialEq, Eq)]
struct MacroArgs {
    /// Raw base58 program id, exactly as written in the attribute.
    program_id: String,
    /// Declared CU charge.
    compute_units: u64,
}

/// Attribute argument list: `program_id = "…", compute_units = 150`.
struct RawArgs(Punctuated<Meta, Comma>);

impl Parse for RawArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        Ok(Self(Punctuated::parse_terminated(input)?))
    }
}

/// Validates and converts the raw attribute into [`MacroArgs`].
fn parse_macro_args(attr: TokenStream) -> syn::Result<MacroArgs> {
    let raw: RawArgs = syn::parse(attr)?;
    let mut program_id: Option<String> = None;
    let mut compute_units: Option<u64> = None;
    for meta in &raw.0 {
        let Meta::NameValue(name_value) = meta else {
            return Err(syn::Error::new_spanned(
                meta,
                "expected `key = value` pairs: \
                 `#[svm_instruction(program_id = \"<base58>\", compute_units = 150)]`",
            ));
        };
        if name_value.path.is_ident("program_id") {
            if program_id.is_some() {
                return Err(syn::Error::new_spanned(
                    meta,
                    "duplicate `program_id`; declare it exactly once",
                ));
            }
            let Expr::Lit(lit) = &name_value.value else {
                return Err(syn::Error::new_spanned(
                    &name_value.value,
                    "`program_id` must be a base58 string literal, e.g. \
                     program_id = \"84LtecYhx229KabnaiMixiuD5Tey4bBo6N4yzZ81Vzpz\"",
                ));
            };
            let Lit::Str(value) = &lit.lit else {
                return Err(syn::Error::new_spanned(
                    &lit.lit,
                    "`program_id` must be a base58 string literal, e.g. \
                     program_id = \"84LtecYhx229KabnaiMixiuD5Tey4bBo6N4yzZ81Vzpz\"",
                ));
            };
            let text = value.value();
            if let Err(reason) = decode_program_id(&text) {
                return Err(syn::Error::new_spanned(value, reason));
            }
            program_id = Some(text);
        } else if name_value.path.is_ident("compute_units") {
            if compute_units.is_some() {
                return Err(syn::Error::new_spanned(
                    meta,
                    "duplicate `compute_units`; declare it exactly once",
                ));
            }
            let Expr::Lit(lit) = &name_value.value else {
                return Err(syn::Error::new_spanned(
                    &name_value.value,
                    "`compute_units` must be an integer literal, e.g. `compute_units = 150`",
                ));
            };
            let Lit::Int(value) = &lit.lit else {
                return Err(syn::Error::new_spanned(
                    &lit.lit,
                    "`compute_units` must be an integer literal, e.g. `compute_units = 150`",
                ));
            };
            compute_units = Some(value.base10_parse::<u64>().map_err(|error| {
                syn::Error::new_spanned(value, format!("invalid `compute_units`: {error}"))
            })?);
        } else {
            return Err(syn::Error::new_spanned(
                &name_value.path,
                "unknown `#[svm_instruction]` argument; expected `program_id` or `compute_units`",
            ));
        }
    }
    let Some(program_id) = program_id else {
        return Err(syn::Error::new_spanned(
            raw.0,
            "missing `program_id`; \
             `#[svm_instruction(program_id = \"<base58>\", compute_units = 150)]`",
        ));
    };
    let Some(compute_units) = compute_units else {
        return Err(syn::Error::new_spanned(
            raw.0,
            "missing `compute_units`; \
             `#[svm_instruction(program_id = \"<base58>\", compute_units = 150)]`",
        ));
    };
    Ok(MacroArgs { program_id, compute_units })
}

/// Decodes a base58 program id, enforcing the 32-byte address length.
fn decode_program_id(text: &str) -> Result<[u8; 32], String> {
    let bytes = bs58::decode(text)
        .into_vec()
        .map_err(|error| format!("`program_id` is not valid base58: {error}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "`program_id` must decode to 32 bytes, decoded to {} bytes",
            bytes.len()
        ));
    }
    let mut id = [0u8; 32];
    id.copy_from_slice(&bytes);
    Ok(id)
}

/// Derives the marker-struct name from the handler function name.
///
/// `move_seven` becomes `MoveSeven`: split on `_`, capitalize each part.
fn struct_name_for(fn_name: &str) -> String {
    fn_name
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}

/// Last path segment of a type, e.g. `SvmInstructionCtx` for `&mut ...::SvmInstructionCtx`.
fn last_segment(ty: &Type) -> Option<String> {
    if let Type::Path(path) = ty {
        path.path.segments.last().map(|segment| segment.ident.to_string())
    } else {
        None
    }
}

/// Enforces the exact handler shape:
/// `fn name(ctx: &mut SvmInstructionCtx) -> Result<(), InstructionError>`.
fn check_handler_shape(func: &ItemFn) -> syn::Result<()> {
    let shape = "expected `fn name(ctx: &mut SvmInstructionCtx) -> Result<(), InstructionError>`";
    if func.sig.asyncness.is_some() {
        return Err(syn::Error::new_spanned(
            func.sig.asyncness,
            format!("`#[svm_instruction]` handlers cannot be async; {shape}"),
        ));
    }
    if !matches!(func.sig.safety, Safety::Default) {
        return Err(syn::Error::new_spanned(
            &func.sig,
            format!("`#[svm_instruction]` handlers cannot be unsafe; {shape}"),
        ));
    }
    if !func.sig.generics.params.is_empty() || func.sig.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &func.sig.generics,
            format!("`#[svm_instruction]` handlers cannot be generic; {shape}"),
        ));
    }
    if func.sig.abi.is_some() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            format!("`#[svm_instruction]` handlers must use the Rust ABI; {shape}"),
        ));
    }
    if func.sig.variadic.is_some() {
        return Err(syn::Error::new_spanned(
            &func.sig,
            format!("`#[svm_instruction]` handlers cannot be variadic; {shape}"),
        ));
    }
    if func.sig.inputs.len() != 1 {
        return Err(syn::Error::new_spanned(
            &func.sig.inputs,
            format!("`#[svm_instruction]` handlers take exactly one argument; {shape}"),
        ));
    }
    let FnArg::Typed(arg) = &func.sig.inputs[0] else {
        return Err(syn::Error::new_spanned(
            &func.sig.inputs[0],
            format!("`#[svm_instruction]` handlers take no receiver; {shape}"),
        ));
    };
    let Type::Reference(reference) = &*arg.ty else {
        return Err(syn::Error::new_spanned(
            &arg.ty,
            format!("handler context must be passed by reference (`&mut`); {shape}"),
        ));
    };
    if reference.mutability.is_none() {
        return Err(syn::Error::new_spanned(
            &arg.ty,
            format!("handler context must be `&mut SvmInstructionCtx`; {shape}"),
        ));
    }
    if last_segment(&reference.elem).as_deref() != Some("SvmInstructionCtx") {
        return Err(syn::Error::new_spanned(
            &reference.elem,
            format!("handler context must be `SvmInstructionCtx`; {shape}"),
        ));
    }
    let ReturnType::Type(_, output) = &func.sig.output else {
        return Err(syn::Error::new_spanned(
            &func.sig.output,
            format!("handlers must return `Result<(), InstructionError>`; {shape}"),
        ));
    };
    let Type::Path(path) = &**output else {
        return Err(syn::Error::new_spanned(
            output,
            format!("handlers must return `Result<(), InstructionError>`; {shape}"),
        ));
    };
    let Some(result) = path.path.segments.last() else {
        return Err(syn::Error::new_spanned(output, format!("unusable return type; {shape}")));
    };
    if result.ident != "Result" {
        return Err(syn::Error::new_spanned(
            output,
            format!("handlers must return `Result<(), InstructionError>`; {shape}"),
        ));
    }
    let PathArguments::AngleBracketed(args) = &result.arguments else {
        return Err(syn::Error::new_spanned(
            output,
            format!("handlers must return `Result<(), InstructionError>`; {shape}"),
        ));
    };
    let mut args = args.args.iter();
    let ok_is_unit = matches!(
        args.next(),
        Some(GenericArgument::Type(Type::Tuple(tuple))) if tuple.elems.is_empty()
    );
    let err_is_instruction_error = matches!(
        args.next(),
        Some(GenericArgument::Type(ty)) if last_segment(ty).as_deref() == Some("InstructionError")
    );
    if !ok_is_unit || !err_is_instruction_error || args.next().is_some() {
        return Err(syn::Error::new_spanned(
            output,
            format!("handlers must return `Result<(), InstructionError>`; {shape}"),
        ));
    }
    Ok(())
}

/// Attribute macro implementation; see the crate docs for the contract.
#[proc_macro_attribute]
pub fn svm_instruction(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = match parse_macro_args(attr) {
        Ok(args) => args,
        Err(error) => return error.to_compile_error().into(),
    };
    let Ok(func) = syn::parse::<ItemFn>(item) else {
        return syn::Error::new(
            Span::call_site(),
            "expected a function \
             `fn name(ctx: &mut SvmInstructionCtx) -> Result<(), InstructionError>`; \
             `#[svm_instruction]` decorates free functions only",
        )
        .to_compile_error()
        .into();
    };
    if let Err(error) = check_handler_shape(&func) {
        return error.to_compile_error().into();
    }

    let Ok(id_bytes) = decode_program_id(&args.program_id) else {
        return syn::Error::new(
            Span::call_site(),
            "internal error: `program_id` passed validation but fails to decode",
        )
        .to_compile_error()
        .into();
    };
    let fn_name = &func.sig.ident;
    let fn_name_text = fn_name.to_string();
    let struct_name = struct_name_for(&fn_name_text);
    let struct_ident = format_ident!("{struct_name}");
    let entrypoint_ident = format_ident!("{struct_name}Entrypoint");
    let compute_units = args.compute_units;
    let output = &func.sig.output;
    let id_literals = id_bytes.iter().map(|byte| quote!(#byte));
    let struct_doc = format!(
        "Zero-sized marker for the `{fn_name_text}` builtin instruction (`{}`).",
        args.program_id
    );
    let program_id_doc =
        format!("Program id declared on `{fn_name_text}` via `#[svm_instruction]`.");
    let cu_doc = format!("Compute units declared on `{fn_name_text}` via `#[svm_instruction]`.");
    let execute_doc =
        format!("Runs the `{fn_name_text}` handler body with bound accounts and data.");
    let register_doc = format!("Installs `{fn_name_text}` as an hpsvm-owned builtin on `svm`.");

    quote!(
        #func

        #[doc = #struct_doc]
        #[derive(Debug, Clone, Copy, Default)]
        pub struct #struct_ident;

        impl #struct_ident {
            #[doc = #program_id_doc]
            pub const PROGRAM_ID: ::solana_address::Address =
                ::solana_address::Address::new_from_array([#(#id_literals),*]);

            #[doc = #cu_doc]
            pub const COMPUTE_UNITS: u64 = #compute_units;

            #[doc = #program_id_doc]
            pub const fn program_id() -> ::solana_address::Address {
                Self::PROGRAM_ID
            }

            #[doc = #execute_doc]
            #[inline]
            pub fn execute(ctx: &mut ::hpsvm::SvmInstructionCtx) #output {
                #fn_name(ctx)
            }

            /// Loader-level registerer installing the entrypoint (macro plumbing).
            #[doc(hidden)]
            pub fn registerer(
                program: &mut ::solana_program_runtime::solana_sbpf::program::BuiltinProgram<
                    ::solana_program_runtime::invoke_context::InvokeContext<'static, 'static>,
                >,
                name: &str,
            ) -> Result<(), ::solana_program_runtime::solana_sbpf::elf::ElfError> {
                program.register_definition::<#entrypoint_ident>(name)
            }

            #[doc = #register_doc]
            pub fn register<T: ::hpsvm::SvmTypesHost>(svm: &mut ::hpsvm::Svm<'_, T>) {
                svm.add_builtin(Self::program_id(), Self::registerer);
            }
        }

        #[doc(hidden)]
        ::solana_program_runtime::solana_sbpf::declare_builtin_function!(
            #entrypoint_ident,
            fn rust(
                invoke_context: &mut ::solana_program_runtime::invoke_context::InvokeContext<'_, '_>,
                _arg0: u64,
                _arg1: u64,
                _arg2: u64,
                _arg3: u64,
                _arg4: u64,
            ) -> Result<u64, Box<dyn ::std::error::Error>> {
                if #struct_ident::COMPUTE_UNITS > 0 {
                    invoke_context.compute_meter.consume_checked(#struct_ident::COMPUTE_UNITS)?;
                }
                let mut ctx = ::hpsvm::SvmInstructionCtx::from_invoke_context(invoke_context)
                    .map_err(|error| Box::new(error) as Box<dyn ::std::error::Error>)?;
                #struct_ident::execute(&mut ctx)
                    .map_err(|error| Box::new(error) as Box<dyn ::std::error::Error>)?;
                ctx.write_back(invoke_context)
                    .map_err(|error| Box::new(error) as Box<dyn ::std::error::Error>)?;
                Ok(0)
            }
        );
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_fn(source: &str) -> ItemFn {
        syn::parse_str(source).expect("test fixture should parse as a function")
    }

    #[test]
    fn accepts_the_exact_handler_shape() {
        let func = parse_fn(
            "fn move_seven(ctx: &mut SvmInstructionCtx) -> Result<(), InstructionError> { Ok(()) }",
        );
        assert!(check_handler_shape(&func).is_ok());
    }

    #[test]
    fn accepts_qualified_paths() {
        let func = parse_fn(
            "fn move_seven(ctx: &mut hpsvm::SvmInstructionCtx) -> \
             std::result::Result<(), solana_transaction::InstructionError> { Ok(()) }",
        );
        assert!(check_handler_shape(&func).is_ok());
    }

    #[test]
    fn rejects_missing_argument() {
        let func = parse_fn("fn bad() -> Result<(), InstructionError> { Ok(()) }");
        assert!(check_handler_shape(&func).is_err());
    }

    #[test]
    fn rejects_shared_reference() {
        let func =
            parse_fn("fn bad(ctx: &SvmInstructionCtx) -> Result<(), InstructionError> { Ok(()) }");
        assert!(check_handler_shape(&func).is_err());
    }

    #[test]
    fn rejects_wrong_context_type() {
        let func =
            parse_fn("fn bad(ctx: &mut OtherCtx) -> Result<(), InstructionError> { Ok(()) }");
        assert!(check_handler_shape(&func).is_err());
    }

    #[test]
    fn rejects_wrong_error_type() {
        let func =
            parse_fn("fn bad(ctx: &mut SvmInstructionCtx) -> Result<(), anyhow::Error> { Ok(()) }");
        assert!(check_handler_shape(&func).is_err());
    }

    #[test]
    fn rejects_missing_return_type() {
        let func = parse_fn("fn bad(ctx: &mut SvmInstructionCtx) { }");
        assert!(check_handler_shape(&func).is_err());
    }

    #[test]
    fn rejects_async_and_generics() {
        let func = parse_fn(
            "async fn bad<T>(ctx: &mut SvmInstructionCtx) -> Result<(), InstructionError> { Ok(()) }",
        );
        assert!(check_handler_shape(&func).is_err());
    }

    #[test]
    fn struct_name_camel_cases_snake_case() {
        assert_eq!(struct_name_for("move_seven"), "MoveSeven");
        assert_eq!(struct_name_for("do_it"), "DoIt");
        assert_eq!(struct_name_for("already"), "Already");
    }

    #[test]
    fn program_id_must_be_32_bytes() {
        assert!(decode_program_id("11111111111111111111111111111111").is_ok());
        assert!(decode_program_id("not-base58!!").is_err());
        assert!(decode_program_id("1111111111111111111111111111111").is_err());
    }
}
