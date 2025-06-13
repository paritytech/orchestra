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

use quote::quote;
use syn::Type;

use super::*;

pub(crate) fn impl_orchestra_struct(info: &OrchestraInfo) -> proc_macro2::TokenStream {
	let message_wrapper = &info.message_wrapper.clone();
	let orchestra_name = info.orchestra_name.clone();
	let subsystem_name = &info.subsystem_names_without_wip();
	let feature_gates = &info.feature_gates();
	let support_crate = info.support_crate_name();

	let baggage_decl = &info.baggage_decl();

	let baggage_generic_ty = &info.baggage_generic_types();

	let generics = quote! {
		< S, #( #baggage_generic_ty, )* >
	};

	let where_clause = quote! {
		where
			S: #support_crate ::Spawner,
	};
	// TODO add `where ..` clauses for baggage types
	// TODO <https://github.com/paritytech/polkadot/issues/3427>

	let consumes = &info.consumes_without_wip();
	let consumes_variant = &info.variant_names_without_wip();
	let unconsumes_variant = &info.variant_names_only_wip();

	let signal_ty = &info.extern_signal_ty;

	let error_ty = &info.extern_error_ty;

	let event_ty = &info.extern_event_ty;

	let signal_channel_capacity = info.signal_channel_capacity;

	let log_target =
		syn::LitStr::new(orchestra_name.to_string().to_lowercase().as_str(), orchestra_name.span());

	let ts = quote! {
		// without `cargo fmt`, there will be some weirdness around else brackets
		// that does not originate from how we create it

		/// Capacity of a signal channel between a subsystem and the orchestra.
		const SIGNAL_CHANNEL_CAPACITY: usize = #signal_channel_capacity;
		/// Timeout to wait for a signal to be processed by the target subsystem. If this timeout is exceeded, the
		/// orchestra terminates with an error.
		const SIGNAL_TIMEOUT: ::std::time::Duration = ::std::time::Duration::from_secs(10);

		/// The log target tag.
		const LOG_TARGET: &str = #log_target;

		/// The orchestra.
		pub struct #orchestra_name #generics {

			#(
				/// A subsystem instance.
				#feature_gates
				#subsystem_name: OrchestratedSubsystem< #consumes >,
			)*

			#(
				/// A user specified addendum field.
				#baggage_decl ,
			)*

			/// Responsible for driving the subsystem futures.
			spawner: S,

			/// The set of running subsystems.
			running_subsystems: #support_crate ::FuturesUnordered<
				BoxFuture<'static, ::std::result::Result<(), #error_ty>>
			>,

			/// Gather running subsystems' outbound streams into one.
			to_orchestra_rx: #support_crate ::stream::Fuse<
				#support_crate ::metered::UnboundedMeteredReceiver< #support_crate ::ToOrchestra >
			>,

			/// Events that are sent to the orchestra from the outside world.
			events_rx: #support_crate ::metered::MeteredReceiver< #event_ty >,
		}

		impl #generics #orchestra_name #generics #where_clause {
			/// Send the given signal, a termination signal, to all subsystems
			/// and wait for all subsystems to go down.
			///
			/// The definition of a termination signal is up to the user and
			/// implementation specific.
			pub async fn wait_terminate(&mut self, signal: #signal_ty, timeout: ::std::time::Duration) -> ::std::result::Result<(), #error_ty > {
				#(
					#feature_gates
					::std::mem::drop(self. #subsystem_name .send_signal(signal.clone()).await);
				)*
				let _ = signal;

				let mut timeout_fut = #support_crate ::Delay::new(
						timeout
					).fuse();

				loop {
					#support_crate ::futures::select! {
						_ = self.running_subsystems.next() =>
						if self.running_subsystems.is_empty() {
							break;
						},
						_ = timeout_fut => break,
						complete => break,
					}
				}

				Ok(())
			}

			/// Broadcast a signal to all subsystems.
			pub async fn broadcast_signal<'a>(&'a mut self, signal: #signal_ty) -> ::std::result::Result<(), #error_ty > {
				let mut delayed_signals : #support_crate ::futures::stream::FuturesUnordered< #support_crate ::futures::future::BoxFuture<'a, ::std::result::Result<(), #error_ty>>>
					= #support_crate ::futures::stream::FuturesUnordered::new();
				#(
					// Use fast path if possible.
					#feature_gates
					if let Err(e) = self. #subsystem_name .try_send_signal(signal.clone()) {
							match e {
								#support_crate::TrySendError::Full(sig) => {
									let instance = self. #subsystem_name .instance.as_mut().expect("checked in try_send_signal");
									delayed_signals.push(::std::boxed::Box::pin(async move {
										match instance.tx_signal.send(sig).timeout(SIGNAL_TIMEOUT).await {
											None => {
												Err(#error_ty :: from(
													#support_crate ::OrchestraError::SubsystemStalled(instance.name, "signal", ::std::any::type_name::<#signal_ty>())
												))
											}
											Some(res) => {
												let res = res.map_err(|_| #error_ty :: from(
													#support_crate ::OrchestraError::QueueError
												));
												if res.is_ok() {
													instance.signals_received += 1;
												}
												res
											}
										}
									}));
								},
								_ => return Err(#error_ty :: from(#support_crate ::OrchestraError::QueueError))
							}
					}
				)*
				let _ = signal;

				// If fast path failed, wait for all delayed signals with no specific order
				while let Some(res) = delayed_signals.next().await {
					res?;
				}

				Ok(())
			}

			/// Route a particular message with normal priority to a subsystem that consumes the message
			pub async fn route_message(&mut self, message: #message_wrapper, origin: &'static str) -> ::std::result::Result<(), #error_ty > {
				self.route_message_with_priority::<#support_crate ::NormalPriority>(message, origin).await
			}

			/// Route a particular message with the specified priority to a subsystem that consumes the message.
			pub async fn route_message_with_priority<P: #support_crate ::Priority>(&mut self, message: #message_wrapper, origin: &'static str) -> ::std::result::Result<(), #error_ty > {
				match message {
					#(
						#feature_gates
						#message_wrapper :: #consumes_variant ( inner ) =>
							OrchestratedSubsystem::< #consumes >::send_message_with_priority::<P>(&mut self. #subsystem_name, inner, origin ).await?,
					)*
					// subsystems that are still work in progress
					#(
						#feature_gates
						#message_wrapper :: #unconsumes_variant ( _ ) => {}
					)*
					#message_wrapper :: Empty => {}

					// And everything that's not WIP but no subsystem consumes it
					#[allow(unreachable_patterns)]
					unused_msg => {
						#support_crate :: tracing :: warn!("Nothing consumes {:?}", unused_msg);
					}
				}
				Ok(())
			}

			/// Extract information from each subsystem.
			pub fn map_subsystems<'a, Mapper, Output>(&'a self, mapper: Mapper)
			-> Vec<Output>
				where
				#(
					Mapper: MapSubsystem<&'a OrchestratedSubsystem< #consumes >, Output=Output>,
				)*
			{
				vec![
				#(
					#feature_gates
					mapper.map_subsystem( & self. #subsystem_name ),
				)*
				]
			}

			/// Get access to internal task spawner.
			pub fn spawner (&mut self) -> &mut S {
				&mut self.spawner
			}
		}

	};

	ts
}

pub(crate) fn impl_orchestrated_subsystem(info: &OrchestraInfo) -> proc_macro2::TokenStream {
	let signal = &info.extern_signal_ty;
	let error_ty = &info.extern_error_ty;
	let support_crate = info.support_crate_name();

	let maybe_boxed_message_generic: Type = if info.boxed_messages {
		parse_quote! { ::std::boxed::Box<M> }
	} else {
		parse_quote! { M }
	};

	let maybe_boxed_message = if info.boxed_messages {
		quote! { ::std::boxed::Box::new(message) }
	} else {
		quote! { message }
	};

	let ts = quote::quote! {
		/// A subsystem that the orchestrator orchestrates.
		///
		/// Ties together the [`Subsystem`] itself and it's running instance
		/// (which may be missing if the [`Subsystem`] is not running at the moment
		/// for whatever reason).
		///
		/// [`Subsystem`]: trait.Subsystem.html
		pub struct OrchestratedSubsystem<M> {
			/// The instance.
			pub instance: std::option::Option<
				#support_crate ::SubsystemInstance<#maybe_boxed_message_generic, #signal>
			>,
		}

		impl<M> OrchestratedSubsystem<M> {

			/// Send a message with normal priority to the wrapped subsystem.
			///
			/// If the inner `instance` is `None`, nothing is happening.
			pub async fn send_message2(&mut self, message: M, origin: &'static str) -> ::std::result::Result<(), #error_ty > {
				self.send_message_with_priority::<#support_crate ::NormalPriority>(message, origin).await
			}


			/// Send a message with specified priority to the wrapped subsystem.
			///
			/// If the inner `instance` is `None`, nothing is happening.
			pub async fn send_message_with_priority<P: #support_crate ::Priority>(&mut self, message: M, origin: &'static str) -> ::std::result::Result<(), #error_ty > {
				const MESSAGE_TIMEOUT: Duration = Duration::from_secs(10);

				if let Some(ref mut instance) = self.instance {
					let send_result = match P::priority() {
						#support_crate ::PriorityLevel::Normal => {
							instance.tx_bounded.send(MessagePacket {
								signals_received: instance.signals_received,
								message: #maybe_boxed_message,
							}).timeout(MESSAGE_TIMEOUT).await
						},
						#support_crate ::PriorityLevel::High => {
							instance.tx_bounded.priority_send(MessagePacket {
								signals_received: instance.signals_received,
								message: #maybe_boxed_message,
							}).timeout(MESSAGE_TIMEOUT).await
						},
					};
					match send_result {
						None => {
							#support_crate ::tracing::error!(
								target: LOG_TARGET,
								%origin,
								"Subsystem {} appears unresponsive when sending a message of type {}.",
								instance.name,
								::std::any::type_name::<M>(),
							);
							Err(#error_ty :: from(
								#support_crate ::OrchestraError::SubsystemStalled(instance.name, "message", ::std::any::type_name::<M>())
							))
						}
						Some(res) => res.map_err(|_| #error_ty :: from(
								#support_crate ::OrchestraError::QueueError
							)),
					}
				} else {
					Ok(())
				}
			}

			/// Tries to send a signal to the wrapped subsystem without waiting.
			pub fn try_send_signal(&mut self, signal: #signal) -> ::std::result::Result<(), #support_crate :: TrySendError<#signal> > {
				if let Some(ref mut instance) = self.instance {
					instance.tx_signal.try_send(signal)?;
					instance.signals_received += 1;
					Ok(())
				} else {
					Ok(())
				}
			}

			/// Send a signal to the wrapped subsystem.
			///
			/// If the inner `instance` is `None`, nothing is happening.
			pub async fn send_signal(&mut self, signal: #signal) -> ::std::result::Result<(), #error_ty > {
				if let Some(ref mut instance) = self.instance {
					match instance.tx_signal.send(signal).timeout(SIGNAL_TIMEOUT).await {
						None => {
							Err(#error_ty :: from(
								#support_crate ::OrchestraError::SubsystemStalled(instance.name, "signal", ::std::any::type_name::<#signal>())
							))
						}
						Some(res) => {
							let res = res.map_err(|_| #error_ty :: from(
								#support_crate ::OrchestraError::QueueError
							));
							if res.is_ok() {
								instance.signals_received += 1;
							}
							res
						}
					}
				} else {
					Ok(())
				}
			}
		}
	};
	ts
}
