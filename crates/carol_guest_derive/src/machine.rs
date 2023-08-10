use crate::activate::HttpMethod;
use proc_macro2::{Ident, Span};
use quote::{format_ident, quote, quote_spanned, ToTokens};
use syn::{
    parse2, parse_quote, parse_quote_spanned,
    punctuated::Punctuated,
    spanned::Spanned,
    token::{self, Brace},
    Arm, Attribute, Expr, ExprMatch, ExprMethodCall, ExprPath, Field, FieldPat, Fields,
    FieldsNamed, FieldsUnnamed, Generics, ImplItem, ItemEnum, ItemStruct, Pat, PatStruct, PatTuple,
    PatTupleStruct, Path, PathSegment, ReturnType, Token, Type, TypePath, Variant, VisPublic,
    Visibility,
};

pub fn machine(input: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let mut input = parse2::<syn::ItemImpl>(input).expect("Can only apply #[carol] to impl");
    let enum_name = format_ident!("Activate");
    let carol_mod = format_ident!("carol_activate");
    let mut call_interface_enum = ItemEnum {
        attrs: vec![
            parse_quote!(#[derive(carol_guest::bincode::Decode, carol_guest::bincode::Encode, Debug, Clone)]),
            parse_quote!(#[bincode(crate = "carol_guest::bincode")]),
        ],
        vis: syn::Visibility::Public(VisPublic {
            pub_token: Token![pub](Span::call_site()),
        }),
        enum_token: Token![enum](Span::call_site()),
        ident: enum_name.clone(),
        generics: Generics::default(),
        brace_token: Brace::default(),
        variants: Punctuated::default(),
    };

    let machine_description = match extract_docs(&input.attrs) {
        Ok(machine_description) => machine_description,
        Err(e) => return e.to_compile_error(),
    };

    let mut match_arms: Vec<Arm> = vec![];
    let mut http_match_arms: Vec<Arm> = vec![];
    let mut method_structs = vec![];
    let mut html_doc_list = crate::html::HttpCallList::default();

    for item in &mut input.items {
        if let ImplItem::Method(method) = item {
            let activate_attr = match method
                .attrs
                .iter()
                .find(|attr| attr.path.to_token_stream().to_string() == "activate")
            {
                Some(activate_attr) => activate_attr,
                None => continue,
            };

            let method_docs = match extract_docs(&method.attrs) {
                Ok(method_docs) => method_docs,
                Err(e) => return e.to_compile_error(),
            };

            let activate_opts = match parse2::<crate::activate::Opts>(activate_attr.tokens.clone())
            {
                Ok(activate_opts) => activate_opts,
                Err(e) => return e.to_compile_error(),
            };

            let method_name = method.sig.ident.clone();
            let sig_span = method_name.span();
            let struct_name = Ident::new(
                &heck::AsUpperCamelCase(&method_name.to_string()).to_string(),
                method.sig.ident.span(),
            );
            let struct_path = Path {
                leading_colon: None,
                segments: Punctuated::from_iter(vec![PathSegment::from(struct_name.clone())]),
            };

            let mut struct_fields = FieldsNamed {
                brace_token: Brace::default(),
                named: Punctuated::default(),
            };

            let mut match_fields = Punctuated::default();
            let mut inputs = method.sig.inputs.iter_mut().peekable();
            let mut _has_receiver = false; //TODO use this to only pass in receiver if it's there
            if let Some(syn::FnArg::Receiver(_)) = inputs.peek() {
                _has_receiver = true;
                let _ = inputs.next();
            } else {
                return quote_spanned!(sig_span => compile_error!("the first argument to an #[activate] method must be &self"));
            }

            if let Some(syn::FnArg::Typed(fn_arg)) = inputs.peek() {
                if let arg @ syn::Pat::Ident(pat_ident) = &*fn_arg.pat {
                    let ident = pat_ident.ident.to_string();
                    if ident == "cap" || ident == "_cap" {
                        let _ = inputs.next();
                    } else {
                        return quote_spanned!(arg.span() => compile_error!("the second argument after `&self` to an #[activate] method must be `cap`"));
                    }
                }
            }
            let mut http_doc_params: Vec<(String, String)> = vec![];
            for fn_arg in inputs {
                let span = fn_arg.span();
                let (field, match_field) = match fn_arg {
                    syn::FnArg::Typed(fn_arg) => match fn_arg.pat.as_mut() {
                        syn::Pat::Ident(pat_ident) => {
                            let mut attrs = vec![];
                            http_doc_params.push((
                                pat_ident.ident.to_string(),
                                fn_arg.ty.to_token_stream().to_string().replace(' ', ""),
                            ));
                            for attr in fn_arg.attrs.drain(..) {
                                if attr.path.get_ident().map(|ident| ident.to_string())
                                    == Some("with_serde".into())
                                {
                                    attrs.push(parse_quote!(#[bincode(with_serde)]));
                                } else {
                                    return quote_spanned!(attr.span() => compile_error!("only 'with_serde' is a valid function argument attribute"));
                                }
                            }
                            (
                                Field {
                                    attrs,
                                    vis: syn::Visibility::Public(VisPublic {
                                        pub_token: Token![pub](span),
                                    }),
                                    ident: Some(pat_ident.ident.clone()),
                                    colon_token: Some(fn_arg.colon_token),
                                    ty: *fn_arg.ty.clone(),
                                },
                                FieldPat {
                                    member: syn::Member::Named(pat_ident.ident.clone()),
                                    colon_token: None,
                                    pat: fn_arg.pat.clone(),
                                    attrs: vec![],
                                },
                            )
                        }
                        arg => {
                            return quote_spanned!(arg.span() => compile_error!("#[activate] only takes plain fn arguments"))
                        }
                    },
                    _ => unreachable!("we dealt with receiver already"),
                };

                struct_fields.named.push(field);
                match_fields.push(match_field);
            }

            let attrs = if activate_opts.http.is_some() {
                vec![
                    parse_quote!(#[derive(carol_guest::bincode::Decode, carol_guest::bincode::Encode, carol_guest::serde::Serialize, carol_guest::serde::Deserialize, Debug, Clone)]),
                    parse_quote!(#[serde(crate = "carol_guest::serde")]),
                    parse_quote!(#[bincode(crate = "carol_guest::bincode")]),
                ]
            } else {
                vec![
                    parse_quote!(#[derive(carol_guest::bincode::Decode, carol_guest::bincode::Encode, Debug, Clone)]),
                    parse_quote!(#[bincode(crate = "carol_guest::bincode")]),
                ]
            };

            let struct_def = ItemStruct {
                attrs,
                vis: syn::Visibility::Public(VisPublic {
                    pub_token: Token![pub](sig_span),
                }),
                struct_token: Token![struct](sig_span),
                ident: struct_name.clone(),
                generics: Generics::default(),
                fields: Fields::Named(struct_fields.clone()),
                semi_token: None,
            };

            method_structs.push(struct_def);

            let variant = Variant {
                attrs: Default::default(),
                ident: struct_name.clone(),
                discriminant: None,
                fields: Fields::Unnamed(FieldsUnnamed {
                    paren_token: token::Paren::default(),
                    unnamed: {
                        Punctuated::from_iter(vec![
                            (Field {
                                attrs: vec![],
                                vis: Visibility::Inherited,
                                ident: None,
                                colon_token: None,
                                ty: Type::Path(TypePath {
                                    qself: None,
                                    path: struct_path.clone(),
                                }),
                            }),
                        ])
                    },
                }),
            };

            let variant_path: Path = parse_quote!(#carol_mod::#enum_name::#struct_name);

            let activate_call = ExprMethodCall {
                attrs: vec![],
                receiver: Box::new(Expr::Verbatim(quote! { machine })),
                dot_token: Token![.](sig_span),
                method: method.sig.ident.clone(),
                turbofish: None,
                paren_token: token::Paren::default(),
                args: {
                    let mut punctuated = Punctuated::new();
                    punctuated.push(parse_quote! { &__ctx });
                    for field in &struct_fields.named {
                        punctuated.push(Expr::Path(ExprPath {
                            attrs: vec![],
                            qself: None,
                            path: Path::from(PathSegment::from(field.ident.clone().unwrap())),
                        }));
                    }
                    punctuated
                },
            };

            let encode_output_expect = format!("Failed to encode output of {}", method_name);
            let encode_output_call = quote_spanned! { sig_span  => carol_guest::bincode::encode_to_vec(#activate_call, carol_guest::bincode::config::standard()).expect(#encode_output_expect) };

            match_arms.push(Arm {
                attrs: vec![],
                pat: Pat::TupleStruct(PatTupleStruct {
                    attrs: vec![],
                    path: variant_path.clone(),
                    pat: PatTuple {
                        attrs: vec![],
                        paren_token: token::Paren::default(),
                        elems: Punctuated::from_iter(vec![Pat::Struct(PatStruct {
                            attrs: vec![],
                            path: Path {
                                leading_colon: None,
                                segments: Punctuated::from_iter(vec![
                                    PathSegment::from(carol_mod.clone()),
                                    PathSegment::from(struct_name.clone()),
                                ]),
                            },
                            brace_token: token::Brace::default(),
                            fields: match_fields,
                            dot2_token: None,
                        })]),
                    },
                }),
                guard: None,
                fat_arrow_token: Token![=>](sig_span),
                body: Box::new(Expr::Verbatim(encode_output_call)),
                comma: None,
            });

            if let Some(http_opt) = activate_opts.http {
                let route_method_ident = match http_opt.method {
                    HttpMethod::Post => format_ident!("Post"),
                    HttpMethod::Get => format_ident!("Get"),
                };
                let (route_path, route_path_str) = match http_opt.path {
                    Some(litstr) => (litstr.clone(), litstr.value()),
                    None => {
                        let default_route = format!("/activate/{}", method.sig.ident);
                        (
                            parse_quote_spanned! { sig_span => #default_route },
                            default_route,
                        )
                    }
                };

                let mut http_doc = crate::html::Call {
                    path: route_path_str,
                    http_method: http_opt.method,
                    docs: method_docs,
                    params: http_doc_params,
                    return_type: None,
                };

                let route_pat = quote_spanned! { sig_span => ( carol_guest::http::Method::#route_method_ident, #route_path ) };
                let struct_path: Path = parse_quote!(#carol_mod::#struct_path);

                let decode_code = match http_opt.method {
                    HttpMethod::Post => {
                        quote_spanned! { sig_span => {
                            carol_guest::serde_json::from_slice::<#struct_path>(body).map_err(|e| format!("{:?}", e))
                        }}
                    }
                    HttpMethod::Get => {
                        quote_spanned! { sig_span => {
                            carol_guest::serde_urlencoded::from_str::<#struct_path>(query).map_err(|e| e.to_string())
                        }}
                    }
                };

                let bincode_encode_error =
                    format!("#[activate] bincode encoding input to {}", method_name);

                let handle_output = match method.sig.output.clone() {
                    ReturnType::Default => {
                        http_doc.return_type = None;
                        quote_spanned! { method.sig.span() => http::Response {
                            headers: vec![],
                            status: 204,
                            body: vec![]
                        }}
                    }
                    ReturnType::Type(_, ty) => {
                        let bincode_decode_output_expect = format!(
                            "#[activate] bincode decoding the output of {} to type {}",
                            method_name,
                            ty.to_token_stream()
                        );
                        let json_encode_output_expect = format!(
                            "#[activate] JSON encoding the output of {} from type {}",
                            method_name,
                            ty.to_token_stream()
                        );
                        http_doc.return_type = Some(ty.to_token_stream().to_string());

                        quote_spanned! { ty.span() =>  {
                            let (decoded_output, _) : (#ty, _) = carol_guest::bincode::decode_from_slice(&output, carol_guest::bincode::config::standard()).expect(#bincode_decode_output_expect);
                            let json_encoded_output = carol_guest::serde_json::to_vec_pretty(&decoded_output).expect(#json_encode_output_expect);
                            http::Response {
                                headers: vec![],
                                status: 200,
                                body: json_encoded_output
                            }
                        }}
                    }
                };

                let arm_body = parse_quote_spanned! { sig_span => {
                    let method_struct = #decode_code;
                    let method_struct = match method_struct {
                        Ok(method_struct) => method_struct,
                        Err(e) => return http::Response {
                            headers: vec![],
                            body: e.as_bytes().to_vec(),
                            status: 400,
                        }
                    };
                    let method_variant = #variant_path(method_struct);
                    let binary_input: Vec<u8> = carol_guest::bincode::encode_to_vec(&method_variant, carol_guest::bincode::config::standard()).expect(#bincode_encode_error);
                    let output = match carol_guest::machines::Cap::self_activate(&__ctx, &binary_input) {
                        Ok(output) => output,
                        Err(e) => return http::Response {
                            headers: vec![],
                            body: format!("HTTP handler failed to self-activate via {}, {}: {}", stringify!(#route_method_ident), #route_path, e).as_bytes().to_vec(),
                            status: 500,
                        }
                    };

                    #handle_output

                }};

                http_match_arms.push(Arm {
                    attrs: vec![],
                    pat: syn::Pat::Verbatim(route_pat),
                    fat_arrow_token: Token![=>](sig_span),
                    body: arm_body,
                    comma: None,
                    guard: None,
                });
                html_doc_list.add_call(http_doc);
            }
            call_interface_enum.variants.push(variant);
        }
    }

    let match_stmt = Expr::Match(ExprMatch {
        attrs: vec![],
        match_token: Token![match](Span::call_site()),
        expr: Box::new(Expr::Verbatim(quote! { method })),
        brace_token: token::Brace::default(),
        arms: match_arms,
    });

    {
        let crate_name = std::env::var("CARGO_PKG_NAME").unwrap_or("<unknown>".to_string());
        let crate_version = std::env::var("CARGO_PKG_VERSION").unwrap_or("<unknown>".to_string());
        let welcome_html_string = crate::html::default_welcome(
            &crate_name,
            &crate_version,
            machine_description.as_deref(),
            &html_doc_list.render(),
        )
        .into_bytes();
        let welcome_literal = proc_macro2::Literal::byte_string(&welcome_html_string);

        http_match_arms.push(parse_quote! { (carol_guest::http::Method::Get, "/") => {
            http::Response {
                headers: vec![],
                body: #welcome_literal.to_vec(),
                status: 200,
            }
        }});
    }

    http_match_arms.push(parse_quote! { _ => {
        http::Response {
            headers: vec![],
            body: vec![],
            status: 404
        }
    }});

    let json_match_stmt = Expr::Match(ExprMatch {
        attrs: vec![],
        match_token: Token![match](Span::call_site()),
        expr: Box::new(Expr::Verbatim(quote! { (request.method, path) })),
        brace_token: token::Brace::default(),
        arms: http_match_arms,
    });

    let self_ty = input.self_ty.clone();
    let params_decode_expect = format!(
        "#[machine] bincode decoding parameters as {}",
        self_ty.to_token_stream().to_string().replace(' ', "")
    );
    let enum_path: Path = parse_quote!(#carol_mod::#enum_name);
    let input_decode_expect = format!(
        "#[machine] bincode decoding input as {}",
        enum_path.to_token_stream().to_string().replace(' ', "")
    );

    let output = quote! {

        pub mod #carol_mod {
            use super::*;
            #(#method_structs)*

            #call_interface_enum
        }

        #[cfg(not(test))]
        carol_guest::set_machine!(#self_ty);

        #input

        mod __machine_impl {
            use super::*;

            #[cfg(target_arch = "wasm32")]
            fn set_up_panic_hook() {
                let original_hook = std::panic::take_hook();
                std::panic::set_hook(Box::new(move |panic_info| {
                    carol_guest::bind::carol::machine::log::set_panic_message(&panic_info.to_string());
                    (original_hook)(panic_info)
                }));
            }

            use carol_guest::{http, bincode};
            impl carol_guest::bind::exports::machine::Machine for #self_ty {
                fn activate(__params: Vec<u8>, __input: Vec<u8>) -> Vec<u8> {
                    #[cfg(target_arch = "wasm32")]
                    set_up_panic_hook();
                    let __ctx = carol_guest::ActivateCap;
                    let (machine, _) = bincode::decode_from_slice::<#self_ty, _>(&__params, bincode::config::standard()).expect(#params_decode_expect);
                    let (method, _) = bincode::decode_from_slice::<#enum_path, _>(&__input, bincode::config::standard()).expect(#input_decode_expect);
                    #match_stmt
                }

                fn handle_http(request: http::Request) -> http::Response {
                    #[cfg(target_arch = "wasm32")]
                    set_up_panic_hook();
                    let __ctx = carol_guest::HttpHandlerCap;

                    let uri = request.uri();
                    let mut path = uri.path();
                    let query = uri.query().unwrap_or("");
                    let body = &request.body;

                    #json_match_stmt
                }
            }
        }

    };

    output
}

fn extract_docs(attrs: &[Attribute]) -> syn::parse::Result<Option<String>> {
    struct Doc(String);

    impl syn::parse::Parse for Doc {
        fn parse(input: syn::parse::ParseStream) -> syn::parse::Result<Self> {
            input.parse::<Token![=]>()?;
            let lit = input.parse::<syn::LitStr>()?;
            Ok(Self(lit.value()))
        }
    }
    let doc_lines = attrs
        .iter()
        .filter(|attr| attr.path.to_token_stream().to_string() == "doc")
        .cloned()
        .map(|doc| parse2::<Doc>(doc.tokens).map(|v| v.0))
        .collect::<Result<Vec<String>, _>>()?;
    if doc_lines.is_empty() {
        Ok(None)
    } else {
        Ok(Some(doc_lines.join("\n")))
    }
}
