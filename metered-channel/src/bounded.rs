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
	channel::mpsc::{Receiver, Sender, TryRecvError, TrySendError as FuturesTrySendError},
	sink::SinkExt,
};

use futures::{
	stream::Stream,
	task::{Context, Poll},
};
use std::{pin::Pin, result};

use super::{prepare_with_tof, MaybeTimeOfFlight, Meter};

/// Create a pair of `MeteredSender` and `MeteredReceiver`. No priorities are provided
pub fn channel<T>(capacity: usize) -> (MeteredSender<T>, MeteredReceiver<T>) {
	let (tx, rx) = bounded_channel::<MaybeTimeOfFlight<T>>(capacity);

	let shared_meter = Meter::default();
	let tx =
		MeteredSender { meter: shared_meter.clone(), bulk_channel: tx, priority_channel: None };
	let rx = MeteredReceiver { meter: shared_meter, bulk_channel: rx, priority_channel: None };
	(tx, rx)
}

/// Create a pair of `MeteredSender` and `MeteredReceiver`. Priority channel is provided
pub fn channel_with_priority<T>(
	capacity_bulk: usize,
	capacity_priority: usize,
) -> (MeteredSender<T>, MeteredReceiver<T>) {
	let (tx, rx) = bounded_channel::<MaybeTimeOfFlight<T>>(capacity_bulk);
	let (tx_pri, rx_pri) = bounded_channel::<MaybeTimeOfFlight<T>>(capacity_priority);

	let shared_meter = Meter::default();
	let tx = MeteredSender {
		meter: shared_meter.clone(),
		bulk_channel: tx,
		priority_channel: Some(tx_pri),
	};
	let rx =
		MeteredReceiver { meter: shared_meter, bulk_channel: rx, priority_channel: Some(rx_pri) };
	(tx, rx)
}

/// A receiver tracking the messages consumed by itself.
#[derive(Debug)]
pub struct MeteredReceiver<T> {
	// count currently contained messages
	meter: Meter,
	bulk_channel: Receiver<MaybeTimeOfFlight<T>>,
	priority_channel: Option<Receiver<MaybeTimeOfFlight<T>>>,
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

#[cfg(feature = "async_channel")]
impl<T> From<ChannelTrySendError<MaybeTimeOfFlight<T>>> for TrySendError<T> {
	fn from(error: ChannelTrySendError<MaybeTimeOfFlight<T>>) -> Self {
		match error {
			ChannelTrySendError::Closed(val) => Self::Closed(val.into()),
			ChannelTrySendError::Full(val) => Self::Full(val.into()),
		}
	}
}

#[cfg(feature = "async_channel")]
impl<T> From<ChannelTrySendError<T>> for TrySendError<T> {
	fn from(error: ChannelTrySendError<T>) -> Self {
		match error {
			ChannelTrySendError::Closed(val) => Self::Closed(val),
			ChannelTrySendError::Full(val) => Self::Full(val),
		}
	}
}

#[cfg(feature = "futures_channel")]
impl<T> From<FuturesTrySendError<MaybeTimeOfFlight<T>>> for TrySendError<T> {
	fn from(error: FuturesTrySendError<MaybeTimeOfFlight<T>>) -> Self {
		let disconnected = error.is_disconnected();
		let val = error.into_inner();
		let val = val.into();
		if disconnected {
			Self::Closed(val)
		} else {
			Self::Full(val)
		}
	}
}

#[cfg(feature = "futures_channel")]
impl<T> From<FuturesTrySendError<T>> for TrySendError<T> {
	fn from(error: FuturesTrySendError<T>) -> Self {
		let disconnected = error.is_disconnected();
		let val = error.into_inner();
		if disconnected {
			Self::Closed(val)
		} else {
			Self::Full(val)
		}
	}
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

	/// Transform the inner value.
	pub fn transform_inner<U, F>(self, f: F) -> TrySendError<U>
	where
		F: FnOnce(T) -> U,
	{
		match self {
			Self::Closed(t) => TrySendError::<U>::Closed(f(t)),
			Self::Full(t) => TrySendError::<U>::Full(f(t)),
		}
	}

	/// Transform the inner value, failable version.
	pub fn try_transform_inner<U, F, E>(self, f: F) -> std::result::Result<TrySendError<U>, E>
	where
		F: FnOnce(T) -> std::result::Result<U, E>,
		E: std::fmt::Debug + std::error::Error + Send + Sync + 'static,
	{
		Ok(match self {
			Self::Closed(t) => TrySendError::<U>::Closed(f(t)?),
			Self::Full(t) => TrySendError::<U>::Full(f(t)?),
		})
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
		&self.bulk_channel
	}
}

impl<T> std::ops::DerefMut for MeteredReceiver<T> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.bulk_channel
	}
}

impl<T> Stream for MeteredReceiver<T> {
	type Item = T;
	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		if let Some(priority_channel) = &mut self.priority_channel {
			match Receiver::poll_next(Pin::new(priority_channel), cx) {
				Poll::Ready(maybe_value) => return Poll::Ready(self.maybe_meter_tof(maybe_value)),
				Poll::Pending => {},
			}
		}
		match Receiver::poll_next(Pin::new(&mut self.bulk_channel), cx) {
			Poll::Ready(maybe_value) => Poll::Ready(self.maybe_meter_tof(maybe_value)),
			Poll::Pending => Poll::Pending,
		}
	}

	/// Don't rely on the unreliable size hint.
	fn size_hint(&self) -> (usize, Option<usize>) {
		self.bulk_channel.size_hint()
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
		// For async_channel we can update channel length in the meter access
		// to avoid more expensive updates on each RW operation
		#[cfg(feature = "async_channel")]
		self.meter.note_channel_len(self.len());

		&self.meter
	}

	/// Attempt to receive the next item.
	/// This function returns:
	///
	///    Ok(Some(t)) when message is fetched
	///    Ok(None) when channel is closed and no messages left in the queue
	///    Err(e) when there are no messages available, but channel is not yet closed
	#[cfg(feature = "futures_channel")]
	pub fn try_next(&mut self) -> Result<Option<T>, TryRecvError> {
		if let Some(priority_channel) = &mut self.priority_channel {
			match priority_channel.try_next() {
				Ok(Some(value)) => return Ok(self.maybe_meter_tof(Some(value))),
				Ok(None) => return Ok(None), // Channel is closed, inform the caller
				Err(_) => {},                // Channel is not closed but empty, ignore the error
			}
		}
		match self.bulk_channel.try_next()? {
			Some(value) => Ok(self.maybe_meter_tof(Some(value))),
			None => Ok(None),
		}
	}

	/// Attempt to receive the next item.
	/// This function returns:
	///
	///    Ok(Some(t)) when message is fetched
	///    Ok(None) when channel is closed and no messages left in the queue
	///    Err(e) when there are no messages available, but channel is not yet closed
	#[cfg(feature = "async_channel")]
	pub fn try_next(&mut self) -> Result<Option<T>, TryRecvError> {
		if let Some(priority_channel) = &mut self.priority_channel {
			match priority_channel.try_recv() {
				Ok(value) => return Ok(self.maybe_meter_tof(Some(value))),
				Err(TryRecvError::Empty) => {},               // Continue to bulk
				Err(TryRecvError::Closed) => return Ok(None), // Mimic futures_channel behaviour
			}
		}
		match self.bulk_channel.try_recv() {
			Ok(value) => Ok(self.maybe_meter_tof(Some(value))),
			Err(TryRecvError::Empty) => Err(TryRecvError::Empty),
			Err(TryRecvError::Closed) => Ok(None), // Mimic futures_channel behaviour
		}
	}

	/// Receive the next item.
	#[cfg(feature = "async_channel")]
	pub async fn recv(&mut self) -> Result<T, RecvError> {
		if let Some(priority_channel) = &mut self.priority_channel {
			match priority_channel.try_recv() {
				Ok(value) =>
					return Ok(self
						.maybe_meter_tof(Some(value))
						.expect("wrapped value is always Some, qed")),
				Err(err) => match err {
					TryRecvError::Closed => return Err(RecvError {}),
					TryRecvError::Empty => {}, // We can still have data in the bulk channel
				},
			}
		}
		match self.bulk_channel.recv().await {
			Ok(value) =>
				Ok(self.maybe_meter_tof(Some(value)).expect("wrapped value is always Some, qed")),
			Err(err) => Err(err.into()),
		}
	}

	/// Attempt to receive the next item without blocking
	#[cfg(feature = "async_channel")]
	pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
		if let Some(priority_channel) = &mut self.priority_channel {
			match priority_channel.try_recv() {
				Ok(value) =>
					return Ok(self
						.maybe_meter_tof(Some(value))
						.expect("wrapped value is always Some, qed")),
				Err(err) => match err {
					TryRecvError::Closed => return Err(err.into()),
					TryRecvError::Empty => {},
				},
			}
		}
		match self.bulk_channel.try_recv() {
			Ok(value) =>
				Ok(self.maybe_meter_tof(Some(value)).expect("wrapped value is always Some, qed")),
			Err(err) => Err(err),
		}
	}

	#[cfg(feature = "async_channel")]
	/// Returns the current number of messages in the channel
	pub fn len(&self) -> usize {
		self.bulk_channel.len() + self.priority_channel.as_ref().map_or(0, |c| c.len())
	}

	#[cfg(feature = "futures_channel")]
	/// Returns the current number of messages in the channel based on meter approximation
	pub fn len(&self) -> usize {
		self.meter.calculate_channel_len()
	}
}

impl<T> futures::stream::FusedStream for MeteredReceiver<T> {
	fn is_terminated(&self) -> bool {
		self.bulk_channel.is_terminated() &&
			self.priority_channel.as_ref().map_or(true, |c| c.is_terminated())
	}
}

/// The sender component, tracking the number of items
/// sent across it.
#[derive(Debug)]
pub struct MeteredSender<T> {
	meter: Meter,
	bulk_channel: Sender<MaybeTimeOfFlight<T>>,
	priority_channel: Option<Sender<MaybeTimeOfFlight<T>>>,
}

impl<T> Clone for MeteredSender<T> {
	fn clone(&self) -> Self {
		Self {
			meter: self.meter.clone(),
			bulk_channel: self.bulk_channel.clone(),
			priority_channel: self.priority_channel.clone(),
		}
	}
}

impl<T> std::ops::Deref for MeteredSender<T> {
	type Target = Sender<MaybeTimeOfFlight<T>>;
	fn deref(&self) -> &Self::Target {
		&self.bulk_channel
	}
}

impl<T> std::ops::DerefMut for MeteredSender<T> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.bulk_channel
	}
}

impl<T> MeteredSender<T> {
	/// Get an updated accessor object for all metrics collected.
	pub fn meter(&self) -> &Meter {
		// For async_channel we can update channel length in the meter access
		// to avoid more expensive updates on each RW operation
		#[cfg(feature = "async_channel")]
		self.meter.note_channel_len(self.len());
		&self.meter
	}

	/// Send message in bulk channel, wait until capacity is available.
	pub async fn send(&mut self, msg: T) -> result::Result<(), SendError<T>>
	where
		Self: Unpin,
	{
		self.send_maybe_priority(msg, false).await
	}

	/// Send message in priority channel (if configured), wait until capacity is available.
	pub async fn send_priority(&mut self, msg: T) -> result::Result<(), SendError<T>>
	where
		Self: Unpin,
	{
		self.send_maybe_priority(msg, true).await
	}

	async fn send_maybe_priority(
		&mut self,
		msg: T,
		is_priority: bool,
	) -> result::Result<(), SendError<T>>
	where
		Self: Unpin,
	{
		let res = if is_priority { self.try_send_priority(msg) } else { self.try_send(msg) };

		match res {
			Err(send_err) => {
				if !send_err.is_full() {
					return Err(SendError::Closed(send_err.into_inner().into()))
				}

				self.meter.note_blocked();
				self.meter.note_sent(); // we are going to do full blocking send, so we have to note it here
				let msg = send_err.into_inner().into();
				self.send_to_channel(msg, is_priority).await
			},
			_ => Ok(()),
		}
	}

	// A helper routine to send a message to the channel after `try_send` returned that a channel is full
	#[cfg(feature = "async_channel")]
	async fn send_to_channel(
		&mut self,
		msg: MaybeTimeOfFlight<T>,
		is_priority: bool,
	) -> result::Result<(), SendError<T>> {
		let channel = if is_priority {
			self.priority_channel.as_mut().unwrap_or(&mut self.bulk_channel)
		} else {
			&mut self.bulk_channel
		};

		let fut = channel.send(msg);
		futures::pin_mut!(fut);
		let result = fut.await.map_err(|err| {
			self.meter.retract_sent();
			SendError::Closed(err.0.into())
		});

		result
	}

	#[cfg(feature = "futures_channel")]
	async fn send_to_channel(
		&mut self,
		msg: MaybeTimeOfFlight<T>,
		is_priority: bool,
	) -> result::Result<(), SendError<T>> {
		let channel = if is_priority {
			self.priority_channel.as_mut().unwrap_or(&mut self.bulk_channel)
		} else {
			&mut self.bulk_channel
		};
		let fut = channel.send(msg);
		futures::pin_mut!(fut);
		fut.await.map_err(|_| {
			self.meter.retract_sent();
			// Futures channel does not provide a way to save the original message,
			// so to avoid `T: Clone` bound we just return a generic error
			SendError::Terminated
		})
	}

	/// Attempt to send message or fail immediately.
	pub fn try_send(&mut self, msg: T) -> result::Result<(), TrySendError<T>> {
		let msg = prepare_with_tof(&self.meter, msg); // note_sent is called in here
		self.bulk_channel.try_send(msg).map_err(|e| {
			self.meter.retract_sent(); // we didn't send it, so we need to undo the note_send
			TrySendError::from(e)
		})
	}

	/// Attempt to send message or fail immediately.
	pub fn try_send_priority(&mut self, msg: T) -> result::Result<(), TrySendError<T>> {
		match self.priority_channel.as_mut() {
			Some(priority_channel) => {
				let msg = prepare_with_tof(&self.meter, msg);
				priority_channel.try_send(msg).map_err(|e| {
					self.meter.retract_sent(); // we didn't send it, so we need to undo the note_send
					TrySendError::from(e)
				})
			},
			None => self.try_send(msg), // use bulk channel as fallback
		}
	}

	#[cfg(feature = "async_channel")]
	/// Returns the current number of messages in the channel
	pub fn len(&self) -> usize {
		self.bulk_channel.len() + self.priority_channel.as_ref().map_or(0, |c| c.len())
	}

	#[cfg(feature = "futures_channel")]
	/// Returns the current number of messages in the channel based on meter approximation
	pub fn len(&self) -> usize {
		self.meter.calculate_channel_len()
	}
}
