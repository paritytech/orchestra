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

use syn::{
	ext::IdentExt,
	parenthesized,
	parse::{Parse, ParseStream},
	parse2,
	punctuated::Punctuated,
	spanned::Spanned,
	token,
	token::Bracket,
	AttrStyle, Error, Field, FieldsNamed, GenericParam, Ident, ItemStruct, LitInt, LitStr, Path,
	PathSegment, Result, Token, Type, Visibility,
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
			CfgItem::All(items) => {
				items.sort();
				items.iter_mut().for_each(|item| item.sort_recursive());
			},
			CfgItem::Any(items) => {
				items.sort();
				items.iter_mut().for_each(|item| item.sort_recursive());
			},
			_ => {},
		}
	}
}

#[derive(Debug, Clone)]
pub struct Feature {
	feature: kw::feature,
	assign: Token![=],
	name: LitStr,
}

impl Parse for Feature {
	fn parse(input: ParseStream) -> Result<Self> {
		Ok(Self { feature: input.parse()?, assign: input.parse()?, name: input.parse()? })
	}
}

#[derive(Debug, Clone)]
pub struct All {
	all: kw::all,
	items: Punctuated<CfgItem, Token![,]>,
}

impl Parse for All {
	fn parse(input: ParseStream) -> Result<Self> {
		let all = input.parse()?;
		let content;
		let _paren_token = parenthesized!(content in input);

		Ok(Self { all, items: Punctuated::parse_terminated(&content)? })
	}
}

#[derive(Debug, Clone)]
pub struct Any {
	any: kw::any,
	items: Punctuated<CfgItem, Token![,]>,
}

impl Parse for Any {
	fn parse(input: ParseStream) -> Result<Self> {
		let any = input.parse()?;
		let content;
		let _paren_token = parenthesized!(content in input);

		Ok(Self { any, items: Punctuated::parse_terminated(&content)? })
	}
}

#[derive(Debug, Clone)]
pub struct Not {
	not: kw::not,
	item: CfgItem,
}

impl Parse for Not {
	fn parse(input: ParseStream) -> Result<Self> {
		let not = input.parse()?;
		let content;
		let _paren_token = parenthesized!(content in input);
		Ok(Self { not, item: content.parse()? })
	}
}

impl Parse for CfgItem {
	fn parse(input: ParseStream) -> Result<Self> {
		let lookahead = input.lookahead1();
		if lookahead.peek(kw::all) {
			let all: All = input.parse()?;
			eprintln!("parsed all {all:?}");
			return Ok(Self::All(Vec::from_iter(all.items.into_iter())))
		} else if lookahead.peek(kw::any) {
			let any: Any = input.parse()?;
			eprintln!("parsed any {any:?}");
			return Ok(Self::Any(Vec::from_iter(any.items.into_iter())))
		} else if lookahead.peek(kw::not) {
			let not: Not = input.parse()?;
			eprintln!("parsed not {not:?}");
			return Ok(Self::Not(not.item.into()))
		} else if lookahead.peek(kw::feature) {
			let feature: Feature = input.parse()?;
			eprintln!("parsed feature {feature:?}");
			return Ok(Self::Feature(feature.name.value()))
		}
		Err(Error::new(input.span(), "Unexpected item in cfg."))
	}
}

impl Parse for CfgExpressionRoot {
	fn parse(input: ParseStream) -> Result<Self> {
		let content;
		let _paren_token = parenthesized!(content in input);

		Ok(Self { item: content.parse()? })
	}
}

#[cfg(test)]
mod test {
	use super::CfgItem::{self, *};
	use assert_matches::assert_matches;
	use syn::parse_quote;

	fn feat(name: &str) -> CfgItem {
		Feature(name.to_string())
	}

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
	fn cfg_item_compares_correctly() {
		let item1: CfgItem = parse_quote! { feature = "bla1"};
		let item2: CfgItem = parse_quote! { feature = "bla1"};
		assert_eq!(true, item1 == item2);

		let item1: CfgItem = parse_quote! { feature = "bla1"};
		let item2: CfgItem = parse_quote! { feature = "bla2"};
		assert_eq!(false, item1 == item2);

		let mut item1: CfgItem = parse_quote! {
			any(
				all(feature = "bla1", feature = "bla2", feature = "bla3"),
				any(feature = "any1", feature = "any2"),
				not(feature = "no")
			)
		};
		let mut item2: CfgItem = parse_quote! {
			any(
				not(feature = "no"),
				any(feature = "any2", feature = "any1"),
				all(feature = "bla2", feature = "bla1", feature = "bla3")
			)
		};
		item1.sort_recursive();
		item2.sort_recursive();
		assert_eq!(true, item1 == item2);
	}
}
