// Copyright 2017-2021 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

#[cfg(feature = "async_channel")]
use async_channel::{
	bounded as bounded_channel, Receiver, Sender, TryRecvError, TrySendError as ChannelTrySendError,
};

#[cfg(feature = "futures_channel")]
use futures::{
	channel::mpsc::channel as bounded_channel,
	channel::mpsc::{Receiver, Sender, TryRecvError},
	sink::SinkExt,
};

use futures::{
	stream::Stream,
	task::{Context, Poll},
};
use std::{pin::Pin, result};

use super::{measure_tof_check, CoarseInstant, MaybeTimeOfFlight, Meter};

/// Create a wrapped `mpsc::channel` pair of `MeteredSender` and `MeteredReceiver`.
pub fn channel<T>(capacity: usize) -> (MeteredSender<T>, MeteredReceiver<T>) {
	let (tx, rx) = bounded_channel::<MaybeTimeOfFlight<T>>(capacity);

	let shared_meter = Meter::default();
	let tx = MeteredSender { meter: shared_meter.clone(), inner: tx };
	let rx = MeteredReceiver { meter: shared_meter, inner: rx };
	(tx, rx)
}

/// A receiver tracking the messages consumed by itself.
#[derive(Debug)]
pub struct MeteredReceiver<T> {
	// count currently contained messages
	meter: Meter,
	inner: Receiver<MaybeTimeOfFlight<T>>,
}

/// A bounded channel error
#[derive(thiserror::Error, Debug)]
pub enum SendError<T> {
	#[error("Bounded channel has been closed")]
	Closed(T),
	#[error("Bounded channel has been closed and the original message is lost")]
	Terminated,
}

impl<T> SendError<T> {
	/// Returns the inner value.
	pub fn into_inner(self) -> Option<T> {
		match self {
			Self::Closed(t) => Some(t),
			Self::Terminated => None,
		}
	}
}

/// A bounded channel error when trying to send a message (transparently wraps the inner error type)
#[derive(thiserror::Error, Debug)]
pub enum TrySendError<T> {
	#[error("Bounded channel has been closed")]
	Closed(T),
	#[error("Bounded channel is full")]
	Full(T),
}

impl<T> TrySendError<T> {
	/// Returns the inner value.
	pub fn into_inner(self) -> T {
		match self {
			Self::Closed(t) => t,
			Self::Full(t) => t,
		}
	}

	/// Returns `true` if we could not send to channel as it was full
	pub fn is_full(&self) -> bool {
		match self {
			Self::Closed(_) => false,
			Self::Full(_) => true,
		}
	}

	/// Returns `true` if we could not send to channel as it was disconnected
	pub fn is_disconnected(&self) -> bool {
		match self {
			Self::Closed(_) => true,
			Self::Full(_) => false,
		}
	}
}

/// Error when receiving from a closed bounded channel
#[derive(thiserror::Error, PartialEq, Eq, Clone, Copy, Debug)]
pub struct RecvError {}

impl std::fmt::Display for RecvError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "receiving from an empty and closed channel")
	}
}

#[cfg(feature = "async_channel")]
impl From<async_channel::RecvError> for RecvError {
	fn from(_: async_channel::RecvError) -> Self {
		RecvError {}
	}
}

impl<T> std::ops::Deref for MeteredReceiver<T> {
	type Target = Receiver<MaybeTimeOfFlight<T>>;
	fn deref(&self) -> &Self::Target {
		&self.inner
	}
}

impl<T> std::ops::DerefMut for MeteredReceiver<T> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.inner
	}
}

impl<T> Stream for MeteredReceiver<T> {
	type Item = T;
	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		match Receiver::poll_next(Pin::new(&mut self.inner), cx) {
			Poll::Ready(maybe_value) => Poll::Ready(self.maybe_meter_tof(maybe_value)),
			Poll::Pending => Poll::Pending,
		}
	}

	/// Don't rely on the unreliable size hint.
	fn size_hint(&self) -> (usize, Option<usize>) {
		self.inner.size_hint()
	}
}

impl<T> MeteredReceiver<T> {
	fn maybe_meter_tof(&mut self, maybe_value: Option<MaybeTimeOfFlight<T>>) -> Option<T> {
		self.meter.note_received();
		maybe_value.map(|value| {
			match value {
				MaybeTimeOfFlight::<T>::WithTimeOfFlight(value, tof_start) => {
					// do not use `.elapsed()` of `std::time`, it may panic
					// `coarsetime` does a saturating sub for all `CoarseInstant` substractions
					let duration = tof_start.elapsed();
					self.meter.note_time_of_flight(duration);
					value
				},
				MaybeTimeOfFlight::<T>::Bare(value) => value,
			}
			.into()
		})
	}

	/// Get an updated accessor object for all metrics collected.
	pub fn meter(&self) -> &Meter {
		&self.meter
	}

	/// Attempt to receive the next item.
	#[cfg(feature = "futures_channel")]
	pub fn try_next(&mut self) -> Result<Option<T>, TryRecvError> {
		match self.inner.try_next()? {
			Some(value) => Ok(self.maybe_meter_tof(Some(value))),
			None => Ok(None),
		}
	}

	/// Attempt to receive the next item.
	#[cfg(feature = "async_channel")]
	pub fn try_next(&mut self) -> Result<Option<T>, TryRecvError> {
		match self.inner.try_recv() {
			Ok(value) => Ok(self.maybe_meter_tof(Some(value))),
			Err(err) => Err(err),
		}
	}

	/// Receive the next item.
	#[cfg(feature = "async_channel")]
	pub async fn recv(&mut self) -> Result<T, RecvError> {
		match self.inner.recv().await {
			Ok(value) =>
				Ok(self.maybe_meter_tof(Some(value)).expect("wrapped value is always Some, qed")),
			Err(err) => Err(err.into()),
		}
	}

	/// Attempt to receive the next item without blocking
	#[cfg(feature = "async_channel")]
	pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
		match self.inner.try_recv() {
			Ok(value) =>
				Ok(self.maybe_meter_tof(Some(value)).expect("wrapped value is always Some, qed")),
			Err(err) => Err(err),
		}
	}

	#[cfg(feature = "async_channel")]
	/// Returns the current number of messages in the channel
	pub fn len(&self) -> usize {
		self.inner.len()
	}
}

impl<T> futures::stream::FusedStream for MeteredReceiver<T> {
	fn is_terminated(&self) -> bool {
		self.inner.is_terminated()
	}
}

/// The sender component, tracking the number of items
/// sent across it.
#[derive(Debug)]
pub struct MeteredSender<T> {
	meter: Meter,
	inner: Sender<MaybeTimeOfFlight<T>>,
}

impl<T> Clone for MeteredSender<T> {
	fn clone(&self) -> Self {
		Self { meter: self.meter.clone(), inner: self.inner.clone() }
	}
}

impl<T> std::ops::Deref for MeteredSender<T> {
	type Target = Sender<MaybeTimeOfFlight<T>>;
	fn deref(&self) -> &Self::Target {
		&self.inner
	}
}

impl<T> std::ops::DerefMut for MeteredSender<T> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.inner
	}
}

impl<T> MeteredSender<T> {
	fn prepare_with_tof(&self, item: T) -> MaybeTimeOfFlight<T> {
		let previous = self.meter.note_sent();
		let item = if measure_tof_check(previous) {
			MaybeTimeOfFlight::WithTimeOfFlight(item, CoarseInstant::now())
		} else {
			MaybeTimeOfFlight::Bare(item)
		};
		item
	}

	/// Get an updated accessor object for all metrics collected.
	pub fn meter(&self) -> &Meter {
		&self.meter
	}

	/// Send message, wait until capacity is available.
	pub async fn send(&mut self, msg: T) -> result::Result<(), SendError<T>>
	where
		Self: Unpin,
	{
		match self.try_send(msg) {
			Err(send_err) => {
				if !send_err.is_full() {
					return Err(SendError::Closed(send_err.into_inner().into()))
				}

				self.meter.note_blocked();
				self.meter.note_sent(); // we are going to do full blocking send, so we have to note it here
				let msg = send_err.into_inner().into();
				self.send_to_channel(msg).await
			},
			_ => Ok(()),
		}
	}

	// A helper routine to send a message to the channel after `try_send` returned that a channel is full
	#[cfg(feature = "async_channel")]
	async fn send_to_channel(
		&mut self,
		msg: MaybeTimeOfFlight<T>,
	) -> result::Result<(), SendError<T>> {
		let fut = self.inner.send(msg);
		futures::pin_mut!(fut);
		fut.await.map_err(|err| {
			self.meter.retract_sent();
			SendError::Closed(err.0.into())
		})
	}

	#[cfg(feature = "futures_channel")]
	async fn send_to_channel(
		&mut self,
		msg: MaybeTimeOfFlight<T>,
	) -> result::Result<(), SendError<T>> {
		let fut = self.inner.send(msg);
		futures::pin_mut!(fut);
		fut.await.map_err(|_| {
			self.meter.retract_sent();
			// Futures channel does not provide a way to save the original message,
			// so to avoid `T: Clone` bound we just return a generic error
			SendError::Terminated
		})
	}

	#[cfg(feature = "futures_channel")]
	/// Attempt to send message or fail immediately.
	pub fn try_send(&mut self, msg: T) -> result::Result<(), TrySendError<T>> {
		let msg = self.prepare_with_tof(msg); // note_sent is called in here
		self.inner.try_send(msg).map_err(|e| {
			self.meter.retract_sent(); // we didn't send it, so we need to undo the note_send
			if e.is_full() {
				TrySendError::Full(e.into_inner().into())
			} else {
				TrySendError::Closed(e.into_inner().into())
			}
		})
	}

	#[cfg(feature = "async_channel")]
	/// Attempt to send message or fail immediately.
	pub fn try_send(&mut self, msg: T) -> result::Result<(), TrySendError<T>> {
		let msg = self.prepare_with_tof(msg); // note_sent is called in here
		self.inner.try_send(msg).map_err(|e| {
			self.meter.retract_sent(); // we didn't send it, so we need to undo the note_send
			match e {
				ChannelTrySendError::Full(inner_error) => TrySendError::Full(inner_error.into()),
				ChannelTrySendError::Closed(inner_error) =>
					TrySendError::Closed(inner_error.into()),
			}
		})
	}

	#[cfg(feature = "async_channel")]
	/// Returns the current number of messages in the channel
	pub fn len(&self) -> usize {
		self.inner.len()
	}
}
