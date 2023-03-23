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

#[derive(Debug, Clone)]
pub enum CfgItem {
	Feature(String),
	All(Vec<CfgItem>),
	Not(Vec<CfgItem>),
	Any(Vec<CfgItem>),
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
	items: Punctuated<CfgItem, Token![,]>,
}

impl Parse for Not {
	fn parse(input: ParseStream) -> Result<Self> {
		let not = input.parse()?;
		let content;
		let _paren_token = parenthesized!(content in input);

		Ok(Self { not, items: Punctuated::parse_terminated(&content)? })
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
			return Ok(Self::Any(Vec::from_iter(not.items.into_iter())))
		} else if lookahead.peek(kw::feature) {
			let feature: Feature = input.parse()?;
			eprintln!("parsed feature {feature:?}");
			return Ok(Self::Feature(feature.name.value()))
		}
		Err(Error::new(input.span(), "yolo"))
	}
}

impl Parse for CfgExpressionRoot {
	fn parse(input: ParseStream) -> Result<Self> {
		let content;
		let _paren_token = parenthesized!(content in input);
		eprintln!("==============================");

		Ok(Self { item: content.parse()? })
	}
}
