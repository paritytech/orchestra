// Copyright (C) 2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use quote::{quote, ToTokens};
use syn::{
	parenthesized,
	parse::{Parse, ParseStream},
	punctuated::Punctuated,
	LitStr, Result, Token,
};

mod kw {
	syn::custom_keyword!(not);
	syn::custom_keyword!(all);
	syn::custom_keyword!(any);
	syn::custom_keyword!(feature);
}

#[derive(Debug, Clone)]
pub(crate) struct CfgExpressionRoot {
	pub(crate) item: CfgItem,
}

impl Parse for CfgExpressionRoot {
	fn parse(input: ParseStream) -> Result<Self> {
		let content;
		let _paren_token = parenthesized!(content in input);

		Ok(Self { item: content.parse()? })
	}
}

#[derive(Debug, Clone, Eq, PartialEq, PartialOrd, Ord)]
pub enum CfgItem {
	Feature(String),
	All(Vec<CfgItem>),
	Not(Box<CfgItem>),
	Any(Vec<CfgItem>),
}

impl CfgItem {
	pub fn sort_recursive(&mut self) {
		match self {
			CfgItem::All(items) | CfgItem::Any(items) => {
				items.sort();
				items.iter_mut().for_each(|item| item.sort_recursive());
			},
			CfgItem::Not(item) => item.sort_recursive(),
			_ => {},
		}
	}
}

impl ToTokens for CfgItem {
	fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
		let ts = match self {
			CfgItem::Feature(name) => quote! { feature = #name },
			CfgItem::All(predicates) => quote! { all(#(#predicates),*) },
			CfgItem::Not(predicate) => quote! { not(#predicate) },
			CfgItem::Any(predicates) => quote! { any(#(#predicates),*) },
		};
		tokens.extend(ts);
	}
}

fn parse_cfg_group<T: Parse>(input: ParseStream) -> Result<Vec<CfgItem>> {
	let _ = input.parse::<T>()?;
	let content;
	let _ = parenthesized!(content in input);
	let cfg_items: Punctuated<CfgItem, Token![,]> = Punctuated::parse_terminated(&content)?;
	Ok(Vec::from_iter(cfg_items))
}

impl Parse for CfgItem {
	fn parse(input: ParseStream) -> Result<Self> {
		let lookahead = input.lookahead1();

		if lookahead.peek(kw::all) {
			let cfg_items = parse_cfg_group::<kw::all>(input)?;

			Ok(Self::All(Vec::from_iter(cfg_items)))
		} else if lookahead.peek(kw::any) {
			let cfg_items = parse_cfg_group::<kw::any>(input)?;

			Ok(Self::Any(Vec::from_iter(cfg_items)))
		} else if lookahead.peek(kw::not) {
			input.parse::<kw::not>()?;
			let content;
			parenthesized!(content in input);

			Ok(Self::Not(content.parse::<CfgItem>()?.into()))
		} else if lookahead.peek(kw::feature) {
			input.parse::<kw::feature>()?;
			input.parse::<Token![=]>()?;

			Ok(Self::Feature(input.parse::<LitStr>()?.value()))
		} else {
			Err(lookahead.error())
		}
	}
}

#[cfg(test)]
mod test {
	use super::CfgItem::{self, *};
	use assert_matches::assert_matches;
	use quote::ToTokens;
	use syn::parse_quote;

	#[test]
	fn cfg_parsing_works() {
		let cfg: CfgItem = parse_quote! {
			feature = "f1"
		};

		assert_matches!(cfg, CfgItem::Feature(item) => {assert_eq!("f1", item)} );

		let cfg: CfgItem = parse_quote! {
			all(feature = "f1", feature = "f2", any(feature = "f3", not(feature = "f4")))
		};

		assert_matches!(cfg, All(all_items) => {
			assert_matches!(all_items.get(0).unwrap(), CfgItem::Feature(item) => {assert_eq!("f1", item)});
			assert_matches!(all_items.get(1).unwrap(), CfgItem::Feature(item) => {assert_eq!("f2", item)});
			assert_matches!(all_items.get(2).unwrap(), CfgItem::Any(any_items) => {
				assert_matches!(any_items.get(0).unwrap(), CfgItem::Feature(item) => {assert_eq!("f3", item)});
				assert_matches!(any_items.get(1).unwrap(), CfgItem::Not(not_item) => {
					assert_matches!(**not_item, CfgItem::Feature(ref inner) => {assert_eq!("f4", inner)})
				});
			});
		});
	}

	#[test]
	fn cfg_item_sorts_and_compares_correctly() {
		let item1: CfgItem = parse_quote! { feature = "f1"};
		let item2: CfgItem = parse_quote! { feature = "f1"};
		assert_eq!(true, item1 == item2);

		let item1: CfgItem = parse_quote! { feature = "f1"};
		let item2: CfgItem = parse_quote! { feature = "f2"};
		assert_eq!(false, item1 == item2);
		let mut item1: CfgItem = parse_quote! {
			any(
				all(feature = "f1", feature = "f2", feature = "f3"),
				any(feature = "any1", feature = "any2"),
				not(feature = "no")
			)
		};
		let mut item2: CfgItem = parse_quote! {
			any(
				not(feature = "no"),
				any(feature = "any2", feature = "any1"),
				all(feature = "f2", feature = "f1", feature = "f3")
			)
		};
		item1.sort_recursive();
		item2.sort_recursive();
		assert_eq!(true, item1 == item2);
	}

	#[test]
	fn cfg_item_is_converted_to_tokens() {
		let to_parse: CfgItem = parse_quote! {
			any(
				not(feature = "no"),
				any(feature = "any2", feature = "any1"),
				all(feature = "f2", feature = "f1", feature = "f3")
			)
		};
		assert_eq!("any (not (feature = \"no\") , any (feature = \"any2\" , feature = \"any1\") , all (feature = \"f2\" , feature = \"f1\" , feature = \"f3\"))"
				   , to_parse.to_token_stream().to_string());
	}
}
