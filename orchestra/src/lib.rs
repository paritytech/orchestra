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

//! # Orchestra
//!
//! `orchestra` provides a global information flow system in reference to system specifc
//! process tokens.
//! The token is arbitrary, but is used to notify all `Subsystem`s of what is relevant
//! and what is not, leading to a so called _view_ of active tokens.
//!
//! An `orchestra` is something that allows spawning/stopping and orchestrating
//! asynchronous tasks as well as establishing a well-defined and easy to use
//! protocol that the tasks can use to communicate with each other. It is desired
//! that this protocol is the only way tasks - called `Subsystem`s in the orchestra
//! scope - communicate with each other, but that is not enforced and is the responsibility
//! of the developer.
//!
//! The `Orchestra` is instantiated with a pre-defined set of `Subsystems` that
//! share the same behavior from `Orchestra`'s point of view.
//!
//! ```text
//!                              +-----------------------------+
//!                              |         Orchesta            |
//!                              +-----------------------------+
//!
//!             ................|  Orchestra "holds" these and uses |.............
//!             .                  them to (re)start things                      .
//!             .                                                                .
//!             .  +-------------------+                +---------------------+  .
//!             .  |   Subsystem1      |                |   Subsystem2        |  .
//!             .  +-------------------+                +---------------------+  .
//!             .           |                                       |            .
//!             ..................................................................
//!                         |                                       |
//!                       start()                                 start()
//!                         |                                       |
//!                         V                                       V
//!             ..................| Orchestra "runs" these |......................
//!             .                                                                .
//!             .  +--------------------+               +---------------------+  .
//!             .  | SubsystemInstance1 | <-- bidir --> | SubsystemInstance2  |  .
//!             .  +--------------------+               +---------------------+  .
//!             .                                                                .
//!             ..................................................................
//! ```
#![deny(unused_results)]
#![deny(missing_docs)]
// #![deny(unused_crate_dependencies)]

pub use orchestra_proc_macro::{contextbounds, orchestra, subsystem};

#[doc(hidden)]
pub use metered;
#[doc(hidden)]
pub use tracing;

#[doc(hidden)]
pub use async_trait::async_trait;
#[doc(hidden)]
pub use futures::{
	self,
	channel::{mpsc, oneshot},
	future::{BoxFuture, Fuse, Future},
	poll, select,
	stream::{self, select, select_with_strategy, FuturesUnordered, PollNext},
	task::{Context, Poll},
	FutureExt, StreamExt,
};
#[doc(hidden)]
pub use std::pin::Pin;

use std::sync::{
	atomic::{self, AtomicUsize},
	Arc,
};
#[doc(hidden)]
pub use std::time::Duration;

#[doc(hidden)]
pub use metered::TrySendError;

#[doc(hidden)]
pub use futures_timer::Delay;

use std::fmt;

#[cfg(test)]
mod tests;

/// A spawner
#[dyn_clonable::clonable]
pub trait Spawner: Clone + Send + Sync {
	/// Spawn the given blocking future.
	///
	/// The given `group` and `name` is used to identify the future in tracing.
	fn spawn_blocking(
		&self,
		name: &'static str,
		group: Option<&'static str>,
		future: futures::future::BoxFuture<'static, ()>,
	);
	/// Spawn the given non-blocking future.
	///
	/// The given `group` and `name` is used to identify the future in tracing.
	fn spawn(
		&self,
		name: &'static str,
		group: Option<&'static str>,
		future: futures::future::BoxFuture<'static, ()>,
	);
}

/// A type of messages that are sent from a [`Subsystem`] to the declared orchestra.
///
/// Used to launch jobs.
pub enum ToOrchestra {
	/// A message that wraps something the `Subsystem` is desiring to
	/// spawn on the orchestra and a `oneshot::Sender` to signal the result
	/// of the spawn.
	SpawnJob {
		/// Name of the task to spawn which be shown in jaeger and tracing logs.
		name: &'static str,
		/// Subsystem of the task to spawn which be shown in jaeger and tracing logs.
		subsystem: Option<&'static str>,
		/// The future to execute.
		s: BoxFuture<'static, ()>,
	},

	/// Same as `SpawnJob` but for blocking tasks to be executed on a
	/// dedicated thread pool.
	SpawnBlockingJob {
		/// Name of the task to spawn which be shown in jaeger and tracing logs.
		name: &'static str,
		/// Subsystem of the task to spawn which be shown in jaeger and tracing logs.
		subsystem: Option<&'static str>,
		/// The future to execute.
		s: BoxFuture<'static, ()>,
	},
}

impl fmt::Debug for ToOrchestra {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::SpawnJob { name, subsystem, .. } => {
				writeln!(f, "SpawnJob{{ {}, {} ..}}", name, subsystem.unwrap_or("default"))
			},
			Self::SpawnBlockingJob { name, subsystem, .. } => {
				writeln!(f, "SpawnBlockingJob{{ {}, {} ..}}", name, subsystem.unwrap_or("default"))
			},
		}
	}
}

/// A helper trait to map a subsystem to smth. else.
pub trait MapSubsystem<T> {
	/// The output type of the mapping.
	type Output;

	/// Consumes a `T` per subsystem, and maps it to `Self::Output`.
	fn map_subsystem(&self, sub: T) -> Self::Output;
}

impl<F, T, U> MapSubsystem<T> for F
where
	F: Fn(T) -> U,
{
	type Output = U;

	fn map_subsystem(&self, sub: T) -> U {
		(self)(sub)
	}
}

/// A wrapping type for messages.
///
/// Includes a counter to synchronize signals with messages,
/// such that no inconsistent message sequences are prevented.
#[derive(Debug)]
pub struct MessagePacket<T> {
	/// Signal level at the point of reception.
	///
	/// Required to assure signals were consumed _before_
	/// consuming messages that are based on the assumption
	/// that a certain signal was assumed.
	pub signals_received: usize,
	/// The message to be sent/consumed.
	pub message: T,
}

/// Create a packet from its parts.
pub fn make_packet<T>(signals_received: usize, message: T) -> MessagePacket<T> {
	MessagePacket { signals_received, message }
}

/// A functor to specify strategy of the channels selection in the `SubsystemIncomingMessages`
pub fn select_message_channel_strategy(_: &mut ()) -> PollNext {
	PollNext::Right
}

/// Incoming messages from both the bounded and unbounded channel.
pub type SubsystemIncomingMessages<M> = self::stream::SelectWithStrategy<
	self::metered::MeteredReceiver<MessagePacket<M>>,
	self::metered::UnboundedMeteredReceiver<MessagePacket<M>>,
	fn(&mut ()) -> self::stream::PollNext,
	(),
>;

/// Watermark to track the received signals.
#[derive(Debug, Default, Clone)]
pub struct SignalsReceived(Arc<AtomicUsize>);

impl SignalsReceived {
	/// Load the current value of received signals.
	pub fn load(&self) -> usize {
		// It's imperative that we prevent reading a stale value from memory because of reordering.
		// Memory barrier to ensure that no reads or writes in the current thread before this load are reordered.
		// All writes in other threads using release semantics become visible to the current thread.
		self.0.load(atomic::Ordering::Acquire)
	}

	/// Increase the number of signals by one.
	pub fn inc(&self) {
		let _previous = self.0.fetch_add(1, atomic::Ordering::AcqRel);
	}
}

/// A trait to support the origin annotation
/// such that errors across subsystems can be easier tracked.
pub trait AnnotateErrorOrigin: 'static + Send + Sync + std::error::Error {
	/// Annotate the error with a origin `str`.
	///
	/// Commonly this is used to create nested enum variants.
	///
	/// ```rust,ignore
	/// E::WithOrigin("I am originally from Cowtown.", E::Variant)
	/// ```
	fn with_origin(self, origin: &'static str) -> Self;
}

/// An asynchronous subsystem task..
///
/// In essence it's just a new type wrapping a `BoxFuture`.
pub struct SpawnedSubsystem<E>
where
	E: std::error::Error + Send + Sync + 'static + From<self::OrchestraError>,
{
	/// Name of the subsystem being spawned.
	pub name: &'static str,
	/// The task of the subsystem being spawned.
	pub future: BoxFuture<'static, Result<(), E>>,
}

/// An error type that describes faults that may happen
///
/// These are:
///   * Channels being closed
///   * Subsystems dying when they are not expected to
///   * Subsystems not dying when they are told to die
///   * etc.
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum OrchestraError {
	#[error(transparent)]
	NotifyCancellation(#[from] oneshot::Canceled),

	#[error("Queue error")]
	QueueError,

	#[error("Failed to spawn task {0}")]
	TaskSpawn(&'static str),

	#[error(transparent)]
	Infallible(#[from] std::convert::Infallible),

	#[error("Failed to {0}")]
	Context(String),

	#[error("Subsystem stalled: {0}, source: {1}, type: {2}")]
	SubsystemStalled(&'static str, &'static str, &'static str),

	/// Per origin (or subsystem) annotations to wrap an error.
	#[error("Error originated in {origin}")]
	FromOrigin {
		/// An additional annotation tag for the origin of `source`.
		origin: &'static str,
		/// The wrapped error. Marked as source for tracking the error chain.
		#[source]
		source: Box<dyn 'static + std::error::Error + Send + Sync>,
	},
}

impl<T> From<metered::SendError<T>> for OrchestraError {
	fn from(_err: metered::SendError<T>) -> Self {
		Self::QueueError
	}
}

/// Alias for a result with error type `OrchestraError`.
pub type OrchestraResult<T> = std::result::Result<T, self::OrchestraError>;

/// Collection of meters related to a subsystem.
#[derive(Clone)]
pub struct SubsystemMeters {
	#[allow(missing_docs)]
	pub bounded: metered::Meter,
	#[allow(missing_docs)]
	pub unbounded: metered::Meter,
	#[allow(missing_docs)]
	pub signals: metered::Meter,
}

impl SubsystemMeters {
	/// Read the values of all subsystem `Meter`s.
	pub fn read(&self) -> SubsystemMeterReadouts {
		SubsystemMeterReadouts {
			bounded: self.bounded.read(),
			unbounded: self.unbounded.read(),
			signals: self.signals.read(),
		}
	}
}

/// Set of readouts of the `Meter`s of a subsystem.
pub struct SubsystemMeterReadouts {
	#[allow(missing_docs)]
	pub bounded: metered::Readout,
	#[allow(missing_docs)]
	pub unbounded: metered::Readout,
	#[allow(missing_docs)]
	pub signals: metered::Readout,
}

/// A running instance of some [`Subsystem`].
///
/// [`Subsystem`]: trait.Subsystem.html
///
/// `M` here is the inner message type, and _not_ the generated `enum AllMessages` or `#message_wrapper` type.
pub struct SubsystemInstance<Message, Signal> {
	/// Send sink for `Signal`s to be sent to a subsystem.
	pub tx_signal: crate::metered::MeteredSender<Signal>,
	/// Send sink for `Message`s to be sent to a subsystem.
	pub tx_bounded: crate::metered::MeteredSender<MessagePacket<Message>>,
	/// All meters of the particular subsystem instance.
	pub meters: SubsystemMeters,
	/// The number of signals already received.
	/// Required to assure messages and signals
	/// are processed correctly.
	pub signals_received: usize,
	/// Name of the subsystem instance.
	pub name: &'static str,
}

/// A message type that a subsystem receives from an orchestra.
/// It wraps signals from an orchestra and messages that are circulating
/// between subsystems.
///
/// It is generic over over the message type `M` that a particular `Subsystem` may use.
#[derive(Debug)]
pub enum FromOrchestra<Message, Signal> {
	/// Signal from the `Orchestra`.
	Signal(Signal),

	/// Some other `Subsystem`'s message.
	Communication {
		/// Contained message
		msg: Message,
	},
}

impl<Signal, Message> From<Signal> for FromOrchestra<Message, Signal> {
	fn from(signal: Signal) -> Self {
		Self::Signal(signal)
	}
}

/// A context type that is given to the [`Subsystem`] upon spawning.
/// It can be used by [`Subsystem`] to communicate with other [`Subsystem`]s
/// or spawn jobs.
///
/// [`Orchestra`]: struct.Orchestra.html
/// [`SubsystemJob`]: trait.SubsystemJob.html
#[async_trait::async_trait]
pub trait SubsystemContext: Send + 'static {
	/// The message type of this context. Subsystems launched with this context will expect
	/// to receive messages of this type. Commonly uses the wrapping `enum` commonly called
	/// `AllMessages`.
	type Message: ::std::fmt::Debug + Send + 'static;
	/// And the same for signals.
	type Signal: ::std::fmt::Debug + Send + 'static;
	/// The overarching messages `enum` for this particular subsystem.
	type OutgoingMessages: ::std::fmt::Debug + Send + 'static;

	// The overarching messages `enum` for this particular subsystem.
	// type AllMessages: From<Self::OutgoingMessages> + From<Self::Message> + std::fmt::Debug + Send + 'static;

	/// The sender type as provided by `sender()` and underlying.
	type Sender: Clone + Send + 'static + SubsystemSender<Self::OutgoingMessages>;
	/// The error type.
	type Error: ::std::error::Error + ::std::convert::From<OrchestraError> + Sync + Send + 'static;

	/// Try to asynchronously receive a message.
	///
	/// Has to be used with caution, if you loop over this without
	/// using `pending!()` macro you will end up with a busy loop!
	async fn try_recv(&mut self) -> Result<Option<FromOrchestra<Self::Message, Self::Signal>>, ()>;

	/// Receive a signal or a message.
	async fn recv(&mut self) -> Result<FromOrchestra<Self::Message, Self::Signal>, Self::Error>;

	/// Receive a signal.
	///
	/// This method allows the subsystem to process signals while being blocked on processing messages.
	/// See `examples/backpressure.rs` for an example.
	async fn recv_signal(&mut self) -> Result<Self::Signal, Self::Error>;

	/// Spawn a child task on the executor.
	fn spawn(
		&mut self,
		name: &'static str,
		s: ::std::pin::Pin<Box<dyn crate::Future<Output = ()> + Send>>,
	) -> Result<(), Self::Error>;

	/// Spawn a blocking child task on the executor's dedicated thread pool.
	fn spawn_blocking(
		&mut self,
		name: &'static str,
		s: ::std::pin::Pin<Box<dyn crate::Future<Output = ()> + Send>>,
	) -> Result<(), Self::Error>;

	/// Send a direct message to some other `Subsystem`, routed based on message type.
	// #[deprecated(note = "Use `self.sender().send_message(msg) instead, avoid passing around the full context.")]
	async fn send_message<T>(&mut self, msg: T)
	where
		Self::OutgoingMessages: From<T> + Send,
		T: Send,
	{
		self.sender().send_message(<Self::OutgoingMessages>::from(msg)).await
	}

	/// Send multiple direct messages to other `Subsystem`s, routed based on message type.
	// #[deprecated(note = "Use `self.sender().send_message(msg) instead, avoid passing around the full context.")]
	async fn send_messages<T, I>(&mut self, msgs: I)
	where
		Self::OutgoingMessages: From<T> + Send,
		I: IntoIterator<Item = T> + Send,
		I::IntoIter: Send,
		T: Send,
	{
		self.sender()
			.send_messages(msgs.into_iter().map(<Self::OutgoingMessages>::from))
			.await
	}

	/// Send a message using the unbounded connection.
	// #[deprecated(note = "Use `self.sender().send_unbounded_message(msg) instead, avoid passing around the full context.")]
	fn send_unbounded_message<X>(&mut self, msg: X)
	where
		Self::OutgoingMessages: From<X> + Send,
		X: Send,
	{
		self.sender().send_unbounded_message(<Self::OutgoingMessages>::from(msg))
	}

	/// Obtain the sender.
	fn sender(&mut self) -> &mut Self::Sender;
}

/// A trait that describes the [`Subsystem`]s that can run on the [`Orchestra`].
///
/// It is generic over the message type circulating in the system.
/// The idea that we want some type containing persistent state that
/// can spawn actually running subsystems when asked.
///
/// [`Orchestra`]: struct.Orchestra.html
/// [`Subsystem`]: trait.Subsystem.html
pub trait Subsystem<Ctx, E>
where
	Ctx: SubsystemContext,
	E: std::error::Error + Send + Sync + 'static + From<self::OrchestraError>,
{
	/// Start this `Subsystem` and return `SpawnedSubsystem`.
	fn start(self, ctx: Ctx) -> SpawnedSubsystem<E>;
}

/// Priority of messages sending to the individual subsystems.
/// Only for the bounded channel sender.

#[derive(Debug)]
pub enum PriorityLevel {
	/// Normal priority.
	Normal,
	/// High priority.
	High,
}
/// Normal priority.
pub struct NormalPriority;
/// High priority.
pub struct HighPriority;

/// Describes the priority of the message.
pub trait Priority {
	/// The priority level.
	fn priority() -> PriorityLevel {
		PriorityLevel::Normal
	}
}
impl Priority for NormalPriority {
	fn priority() -> PriorityLevel {
		PriorityLevel::Normal
	}
}
impl Priority for HighPriority {
	fn priority() -> PriorityLevel {
		PriorityLevel::High
	}
}

/// Sender end of a channel to interface with a subsystem.
#[async_trait::async_trait]
pub trait SubsystemSender<OutgoingMessage>: Clone + Send + 'static
where
	OutgoingMessage: Send,
{
	/// Send a direct message to some other `Subsystem`, routed based on message type.
	async fn send_message(&mut self, msg: OutgoingMessage);

	/// Send a direct message with defined priority to some other `Subsystem`, routed based on message type.
	async fn send_message_with_priority<P: Priority>(&mut self, msg: OutgoingMessage);

	/// Tries to send a direct message to some other `Subsystem`, routed based on message type.
	/// This method is useful for cases where the message queue is bounded and the message is ok
	/// to be dropped if the queue is full. If the queue is full, this method will return an error.
	/// This method is not async and will not block the current task.
	fn try_send_message(
		&mut self,
		msg: OutgoingMessage,
	) -> Result<(), metered::TrySendError<OutgoingMessage>>;

	/// Tries to send a direct message with defined priority to some other `Subsystem`, routed based on message type.
	/// If the queue is full, this method will return an error.
	/// This method is not async and will not block the current task.
	fn try_send_message_with_priority<P: Priority>(
		&mut self,
		msg: OutgoingMessage,
	) -> Result<(), metered::TrySendError<OutgoingMessage>>;

	/// Send multiple direct messages to other `Subsystem`s, routed based on message type.
	async fn send_messages<I>(&mut self, msgs: I)
	where
		I: IntoIterator<Item = OutgoingMessage> + Send,
		I::IntoIter: Send;

	/// Send a message onto the unbounded queue of some other `Subsystem`, routed based on message
	/// type.
	///
	/// This function should be used only when there is some other bounding factor on the messages
	/// sent with it. Otherwise, it risks a memory leak.
	fn send_unbounded_message(&mut self, msg: OutgoingMessage);
}

/// A future that wraps another future with a `Delay` allowing for time-limited futures.
#[pin_project::pin_project]
pub struct Timeout<F: Future> {
	#[pin]
	future: F,
	#[pin]
	delay: Delay,
}

/// Extends `Future` to allow time-limited futures.
pub trait TimeoutExt: Future {
	/// Adds a timeout of `duration` to the given `Future`.
	/// Returns a new `Future`.
	fn timeout(self, duration: Duration) -> Timeout<Self>
	where
		Self: Sized,
	{
		Timeout { future: self, delay: Delay::new(duration) }
	}
}

impl<F> TimeoutExt for F where F: Future {}

impl<F> Future for Timeout<F>
where
	F: Future,
{
	type Output = Option<F::Output>;

	fn poll(self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Self::Output> {
		let this = self.project();

		if this.delay.poll(ctx).is_ready() {
			return Poll::Ready(None)
		}

		if let Poll::Ready(output) = this.future.poll(ctx) {
			return Poll::Ready(Some(output))
		}

		Poll::Pending
	}
}
