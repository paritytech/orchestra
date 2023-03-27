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

/// Top-level cfg expression item
#[derive(Debug, Clone)]
pub(crate) struct CfgExpressionRoot {
	pub(crate) predicate: CfgPredicate,
}

impl Parse for CfgExpressionRoot {
	fn parse(input: ParseStream) -> Result<Self> {
		let content;
		let _paren_token = parenthesized!(content in input);

		Ok(Self { predicate: content.parse()? })
	}
}

/// Single cfg predicate for parsing.
#[derive(Debug, Clone, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub(crate) enum CfgPredicate {
	Feature(String),
	All(Vec<CfgPredicate>),
	Not(Box<CfgPredicate>),
	Any(Vec<CfgPredicate>),
}

impl CfgPredicate {
	pub(crate) fn sort_recursive(&mut self) {
		match self {
			CfgPredicate::All(predicates) | CfgPredicate::Any(predicates) => {
				predicates.sort();
				predicates.iter_mut().for_each(|p| p.sort_recursive());
			},
			CfgPredicate::Not(p) => p.sort_recursive(),
			_ => {},
		}
	}
}

impl ToTokens for CfgPredicate {
	fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
		let ts = match self {
			CfgPredicate::Feature(name) => quote! { feature = #name },
			CfgPredicate::All(predicates) => quote! { all(#(#predicates),*) },
			CfgPredicate::Not(predicate) => quote! { not(#predicate) },
			CfgPredicate::Any(predicates) => quote! { any(#(#predicates),*) },
		};
		tokens.extend(ts);
	}
}

fn parse_cfg_group<T: Parse>(input: ParseStream) -> Result<Vec<CfgPredicate>> {
	let _ = input.parse::<T>()?;
	let content;
	let _ = parenthesized!(content in input);
	let cfg_items: Punctuated<CfgPredicate, Token![,]> = Punctuated::parse_terminated(&content)?;
	Ok(Vec::from_iter(cfg_items))
}

impl Parse for CfgPredicate {
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

			Ok(Self::Not(content.parse::<CfgPredicate>()?.into()))
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
	use super::CfgPredicate::{self, *};
	use assert_matches::assert_matches;
	use quote::ToTokens;
	use syn::parse_quote;

	#[test]
	fn cfg_parsing_works() {
		let cfg: CfgPredicate = parse_quote! {
			feature = "f1"
		};

		assert_matches!(cfg, CfgPredicate::Feature(item) => {assert_eq!("f1", item)} );

		let cfg: CfgPredicate = parse_quote! {
			all(feature = "f1", feature = "f2", any(feature = "f3", not(feature = "f4")))
		};

		assert_matches!(cfg, All(all_items) => {
			assert_matches!(all_items.get(0).unwrap(), CfgPredicate::Feature(item) => {assert_eq!("f1", item)});
			assert_matches!(all_items.get(1).unwrap(), CfgPredicate::Feature(item) => {assert_eq!("f2", item)});
			assert_matches!(all_items.get(2).unwrap(), CfgPredicate::Any(any_items) => {
				assert_matches!(any_items.get(0).unwrap(), CfgPredicate::Feature(item) => {assert_eq!("f3", item)});
				assert_matches!(any_items.get(1).unwrap(), CfgPredicate::Not(not_item) => {
					assert_matches!(**not_item, CfgPredicate::Feature(ref inner) => {assert_eq!("f4", inner)})
				});
			});
		});
	}

	#[test]
	fn cfg_item_sorts_and_compares_correctly() {
		let item1: CfgPredicate = parse_quote! { feature = "f1"};
		let item2: CfgPredicate = parse_quote! { feature = "f1"};
		assert_eq!(true, item1 == item2);

		let item1: CfgPredicate = parse_quote! { feature = "f1"};
		let item2: CfgPredicate = parse_quote! { feature = "f2"};
		assert_eq!(false, item1 == item2);
		let mut item1: CfgPredicate = parse_quote! {
			any(
				all(feature = "f1", feature = "f2", feature = "f3"),
				any(feature = "any1", feature = "any2"),
				not(feature = "no")
			)
		};
		let mut item2: CfgPredicate = parse_quote! {
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
		let to_parse: CfgPredicate = parse_quote! {
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
