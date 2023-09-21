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

//! Metered variant of mpsc channels to be able to extract metrics.
#![allow(clippy::all)]

#[cfg(all(feature = "async_channel", feature = "futures_channel",))]
compile_error!("`async_channel` and `futures_channel` are mutually exclusive features");

#[cfg(not(any(feature = "async_channel", feature = "futures_channel")))]
compile_error!("Must build with either `async_channel` or `futures_channel` features");

use std::sync::{
	atomic::{AtomicUsize, Ordering},
	Arc,
};

use derive_more::Display;

mod bounded;
pub mod oneshot;
mod unbounded;

pub use self::{bounded::*, unbounded::*};

pub use coarsetime::Duration as CoarseDuration;
use coarsetime::Instant as CoarseInstant;

#[cfg(test)]
mod tests;

/// Defines the maximum number of time of flight values to be stored.
const TOF_QUEUE_SIZE: usize = 100;

/// A peek into the inner state of a meter.
#[derive(Debug, Clone)]
pub struct Meter {
	// Number of sends on this channel.
	sent: Arc<AtomicUsize>,
	// Number of receives on this channel.
	received: Arc<AtomicUsize>,
	#[cfg(feature = "async_channel")]
	// Number of elements in the channel.
	channel_len: Arc<AtomicUsize>,
	// Number of times senders blocked while sending messages to a subsystem.
	blocked: Arc<AtomicUsize>,
	// Atomic ringbuffer of the last `TOF_QUEUE_SIZE` time of flight values
	tof: Arc<crossbeam_queue::ArrayQueue<CoarseDuration>>,
}

impl std::default::Default for Meter {
	fn default() -> Self {
		Self {
			sent: Arc::new(AtomicUsize::new(0)),
			received: Arc::new(AtomicUsize::new(0)),
			#[cfg(feature = "async_channel")]
			channel_len: Arc::new(AtomicUsize::new(0)),
			blocked: Arc::new(AtomicUsize::new(0)),
			tof: Arc::new(crossbeam_queue::ArrayQueue::new(TOF_QUEUE_SIZE)),
		}
	}
}

/// A readout of sizes from the meter. Note that it is possible, due to asynchrony, for received
/// to be slightly higher than sent.
#[derive(Debug, Display, Clone, Default, PartialEq)]
#[display(fmt = "(sent={} received={})", sent, received)]
pub struct Readout {
	/// The amount of messages sent on the channel, in aggregate.
	pub sent: usize,
	/// The amount of messages received on the channel, in aggregate.
	pub received: usize,
	/// An approximation of the queue size.
	pub channel_len: usize,
	/// How many times the caller blocked when sending messages.
	pub blocked: usize,
	/// Time of flight in micro seconds (us)
	pub tof: Vec<CoarseDuration>,
}

impl Meter {
	/// Count the number of items queued up inside the channel.
	pub fn read(&self) -> Readout {
		// when obtaining we don't care much about off by one
		// accuracy
		let sent = self.sent.load(Ordering::Relaxed);
		let received = self.received.load(Ordering::Relaxed);

		#[cfg(feature = "async_channel")]
		let channel_len = self.channel_len.load(Ordering::Relaxed);
		#[cfg(feature = "futures_channel")]
		let channel_len = sent.saturating_sub(received);

		Readout {
			sent,
			received,
			channel_len,
			blocked: self.blocked.load(Ordering::Relaxed),
			tof: {
				let mut acc = Vec::with_capacity(self.tof.len());
				while let Some(value) = self.tof.pop() {
					acc.push(value)
				}
				acc
			},
		}
	}

	fn note_sent(&self) -> usize {
		self.sent.fetch_add(1, Ordering::Relaxed)
	}

	#[cfg(feature = "async_channel")]
	fn note_channel_len(&self, len: usize) {
		self.channel_len.store(len, Ordering::Relaxed)
	}

	fn retract_sent(&self) {
		self.sent.fetch_sub(1, Ordering::Relaxed);
	}

	fn note_received(&self) {
		self.received.fetch_add(1, Ordering::Relaxed);
	}

	fn note_blocked(&self) {
		self.blocked.fetch_add(1, Ordering::Relaxed);
	}

	fn note_time_of_flight(&self, tof: CoarseDuration) {
		let _ = self.tof.force_push(tof);
	}

	#[cfg(feature = "futures_channel")]
	fn calculate_channel_len(&self) -> usize {
		let sent = self.sent.load(Ordering::Relaxed);
		let received = self.received.load(Ordering::Relaxed);
		sent.saturating_sub(received) as usize
	}
}

/// Determine if this instance shall be measured
#[inline(always)]
fn measure_tof_check(nth: usize) -> bool {
	if cfg!(test) {
		// for tests, be deterministic and pick every second
		nth & 0x01 == 0
	} else {
		use nanorand::Rng;
		let mut rng = nanorand::WyRand::new_seed(nth as u64);
		let pick = rng.generate_range(1_usize..=1000);
		// measure 5.3%
		pick <= 53
	}
}

/// Measure the time of flight between insertion and removal
/// of a single type `T`

#[derive(Debug)]
pub enum MaybeTimeOfFlight<T: Sized> {
	Bare(T),
	WithTimeOfFlight(T, CoarseInstant),
}

impl<T> From<T> for MaybeTimeOfFlight<T> {
	fn from(value: T) -> Self {
		Self::Bare(value)
	}
}

// Has some unexplicable conflict with a wildcard impl of std
impl<T> MaybeTimeOfFlight<T> {
	/// Extract the inner `T` value.
	pub fn into(self) -> T {
		match self {
			Self::Bare(value) => value,
			Self::WithTimeOfFlight(value, _tof_start) => value,
		}
	}
}

impl<T> std::ops::Deref for MaybeTimeOfFlight<T> {
	type Target = T;
	fn deref(&self) -> &Self::Target {
		match self {
			Self::Bare(ref value) => value,
			Self::WithTimeOfFlight(ref value, _tof_start) => value,
		}
	}
}
