use crate::utils::salvo_crate;
use proc_macro2::{Span, TokenStream};
use quote::{quote, ToTokens};
use syn::{
    parse::Parser, parse_quote, Attribute, FnArg, Ident, ImplItem, ImplItemFn, Item, Token, Type,
};

pub(crate) fn generate(input: Item) -> syn::Result<TokenStream> {
    match input {
        Item::Impl(mut item_impl) => {
            for item in &mut item_impl.items {
                if let ImplItem::Fn(method) = item {
                    rewrite_method(item_impl.self_ty.clone(), method)?
                }
            }
            Ok(item_impl.into_token_stream())
        }
        Item::Fn(_) => {
            return Ok(input.into_token_stream());
        }
        _ => Err(syn::Error::new_spanned(
            input,
            "#[module] must added to `impl`",
        )),
    }
}

fn take_method_macro(item_fn: &mut ImplItemFn) -> syn::Result<Option<Attribute>> {
    let mut index: Option<usize> = None;
    let mut new_attr: Option<Attribute> = None;
    for (idx, attr) in &mut item_fn.attrs.iter().enumerate() {
        if !match attr.path().segments.last() {
            Some(segment) => segment.ident.to_string() == "module",
            None => false,
        } {
            continue;
        }
        if let Some((_, last)) = attr.to_token_stream().to_string().split_once("module(") {
            let last = last.strip_suffix(")]");
            if last.is_none() {
                continue;
            }
            let last = last.unwrap();
            let ts: TokenStream = if last == "handler" || last.starts_with("handler(") {
                format!("#[{}::{last}]", salvo_crate()).parse()?
            } else if last == "endpoint" || last.starts_with("endpoint(") {
                format!("#[{}::oapi::{last}]", salvo_crate()).parse()?
            } else {
                continue;
            };
            new_attr = Attribute::parse_outer.parse2(ts)?.into_iter().next();
            index = Some(idx);
        }
    }
    if let Some(index) = index {
        item_fn.attrs.remove(index);
        return Ok(new_attr);
    }
    Ok(None)
}

enum MethodStyle {
    NoSelf,
    RefSelf,
    ArcSelf,
}

impl MethodStyle {
    fn from_method(method: &ImplItemFn) -> syn::Result<Self> {
        let Some(recv) = method.sig.receiver() else {
            return Ok(Self::NoSelf);
        };
        let ty = recv.ty.to_token_stream().to_string().replace(" ", "");
        match ty.as_str() {
            "&Self" => Ok(Self::RefSelf),
            "Arc<Self>" | "&Arc<Self>" => Ok(Self::ArcSelf),
            _ => {
                if ty.ends_with("::Arc<Self>") {
                    Ok(Self::ArcSelf)
                } else {
                    Err(syn::Error::new_spanned(
                        method,
                        "#[module] method receiver must be '&self', 'Arc<Self>' or '&Arc<Self>'",
                    ))
                }
            }
        }
    }
}

fn rewrite_method(self_ty: Box<Type>, method: &mut ImplItemFn) -> syn::Result<()> {
    let Some(macro_attr) = take_method_macro(method)? else {
        return Ok(());
    };
    method.sig.asyncness = Some(Token![async](Span::call_site()));
    let salvo = salvo_crate();
    let handler = quote! (#salvo::Handler);
    let method_name = method.sig.ident.clone();
    let vis = method.vis.clone();
    let mut attrs = method.attrs.clone();
    let mut new_method: ImplItemFn = match MethodStyle::from_method(method)? {
        MethodStyle::NoSelf => {
            method.attrs.push(macro_attr);
            parse_quote! {
                #vis fn #method_name() -> impl #handler {

                    #method

                    #method_name
                }
            }
        }
        style => {
            let (receiver, output) = match style {
                MethodStyle::RefSelf => {
                    (quote!(&self), quote!(::std::sync::Arc::new(self.clone())))
                }
                MethodStyle::ArcSelf => {
                    (quote!(self: &::std::sync::Arc<Self>), quote!(self.clone()))
                }
                _ => unreachable!(),
            };
            method.sig.inputs[0] = FnArg::Receiver(parse_quote!(&self));
            method.sig.ident = Ident::new("handle", Span::call_site());
            parse_quote! {
                #vis fn #method_name(#receiver) -> impl #handler {
                    pub struct handle(::std::sync::Arc<#self_ty>);
                    impl ::std::ops::Deref for handle {
                        type Target = #self_ty;

                        fn deref(&self) -> &Self::Target {
                            &self.0
                        }
                    }
                    #macro_attr
                    impl handle {
                        #method
                    }
                    handle(#output)
                }
            }
        }
    };
    new_method.attrs.append(&mut attrs);
    *method = new_method;
    Ok(())
}