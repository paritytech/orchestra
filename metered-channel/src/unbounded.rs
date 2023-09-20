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

//! Metered variant of unbounded mpsc channels to be able to extract metrics.

use futures::{
	stream::Stream,
	task::{Context, Poll},
};

#[cfg(feature = "async_channel")]
use async_channel::{unbounded as unbounded_channel, Receiver, Sender, TryRecvError, TrySendError};

#[cfg(feature = "futures_channel")]
use futures::{
	channel::mpsc::unbounded as unbounded_channel,
	channel::mpsc::{
		TryRecvError, TrySendError, UnboundedReceiver as Receiver, UnboundedSender as Sender,
	},
};

use std::{pin::Pin, result};

use super::{measure_tof_check, CoarseInstant, MaybeTimeOfFlight, Meter};

/// Create a wrapped `mpsc::channel` pair of `MeteredSender` and `MeteredReceiver`.
pub fn unbounded<T>() -> (UnboundedMeteredSender<T>, UnboundedMeteredReceiver<T>) {
	let (tx, rx) = unbounded_channel::<MaybeTimeOfFlight<T>>();
	let shared_meter = Meter::default();
	let tx = UnboundedMeteredSender { meter: shared_meter.clone(), inner: tx };
	let rx = UnboundedMeteredReceiver { meter: shared_meter, inner: rx };
	(tx, rx)
}

/// A receiver tracking the messages consumed by itself.
#[derive(Debug)]
pub struct UnboundedMeteredReceiver<T> {
	// count currently contained messages
	meter: Meter,
	inner: Receiver<MaybeTimeOfFlight<T>>,
}

impl<T> std::ops::Deref for UnboundedMeteredReceiver<T> {
	type Target = Receiver<MaybeTimeOfFlight<T>>;
	fn deref(&self) -> &Self::Target {
		&self.inner
	}
}

impl<T> std::ops::DerefMut for UnboundedMeteredReceiver<T> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.inner
	}
}

impl<T> Stream for UnboundedMeteredReceiver<T> {
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

impl<T> UnboundedMeteredReceiver<T> {
	fn maybe_meter_tof(&mut self, maybe_value: Option<MaybeTimeOfFlight<T>>) -> Option<T> {
		self.meter.note_received();
		let result = maybe_value.map(|value| {
			match value {
				MaybeTimeOfFlight::<T>::WithTimeOfFlight(value, tof_start) => {
					// do not use `.elapsed()` of `std::time`, it may panic
					// `coarsetime` does a saturating substractio for all `CoarseInstant`s
					let duration = tof_start.elapsed();
					self.meter.note_time_of_flight(duration);
					value
				},
				MaybeTimeOfFlight::<T>::Bare(value) => value,
			}
			.into()
		});

		result
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
		let result = match self.inner.try_recv() {
			Ok(value) => Ok(self.maybe_meter_tof(Some(value))),
			Err(err) => Err(err),
		};

		self.meter.note_channel_len(self.len());
		result
	}

	/// Returns the current number of messages in the channel
	#[cfg(feature = "async_channel")]
	pub fn len(&self) -> usize {
		self.inner.len()
	}
}

impl<T> futures::stream::FusedStream for UnboundedMeteredReceiver<T> {
	fn is_terminated(&self) -> bool {
		self.inner.is_terminated()
	}
}

/// The sender component, tracking the number of items
/// sent across it.
#[derive(Debug)]
pub struct UnboundedMeteredSender<T> {
	meter: Meter,
	inner: Sender<MaybeTimeOfFlight<T>>,
}

impl<T> Clone for UnboundedMeteredSender<T> {
	fn clone(&self) -> Self {
		Self { meter: self.meter.clone(), inner: self.inner.clone() }
	}
}

impl<T> std::ops::Deref for UnboundedMeteredSender<T> {
	type Target = Sender<MaybeTimeOfFlight<T>>;
	fn deref(&self) -> &Self::Target {
		&self.inner
	}
}

impl<T> std::ops::DerefMut for UnboundedMeteredSender<T> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.inner
	}
}

impl<T> UnboundedMeteredSender<T> {
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

	/// Attempt to send message or fail immediately.
	#[cfg(feature = "futures_channel")]
	pub fn unbounded_send(&self, msg: T) -> result::Result<(), TrySendError<MaybeTimeOfFlight<T>>> {
		let msg = self.prepare_with_tof(msg);
		self.inner.unbounded_send(msg).map_err(|e| {
			self.meter.retract_sent();
			e
		})
	}

	/// Attempt to send message or fail immediately.
	#[cfg(feature = "async_channel")]
	pub fn unbounded_send(&self, msg: T) -> result::Result<(), TrySendError<MaybeTimeOfFlight<T>>> {
		let msg = self.prepare_with_tof(msg);
		let result = self.inner.try_send(msg).map_err(|e| {
			self.meter.retract_sent();
			e
		});

		self.meter.note_channel_len(self.len());
		result
	}

	/// Returns the current number of messages in the channel
	#[cfg(feature = "async_channel")]
	pub fn len(&self) -> usize {
		self.inner.len()
	}
}
