// Copyright (C) 2023 Parity Technologies (UK) Ltd.
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

#![allow(clippy::all)]

use criterion::{criterion_group, Criterion};
use futures::{executor, executor::ThreadPool, future::try_join_all};
use orchestra::{self as orchestra, Spawner, *};
use std::time::Instant;

pub use super::misc::*;

#[derive(Default)]
pub struct SenderSubsys(pub u64);

#[derive(Default)]
pub struct ReceiverSubsys(pub u64);

#[orchestra(signal=SigSigSig, event=EvX, error=Yikes, gen=AllMessages, boxed_messages=true)]
struct BenchBoxedLarge {
	#[subsystem(sends: [Msg1029], consumes: MsgU64, message_capacity: 60000)]
	sender: Sender,

	#[subsystem(blocking, consumes: Msg1029, sends: [MsgU64], message_capacity: 60000, signal_capacity: 128)]
	receiver: Receiver,
}

#[orchestra::subsystem(Sender, error=Yikes)]
impl<Context> SenderSubsys {
	fn start(self, mut ctx: Context) -> SpawnedSubsystem<Yikes> {
		let mut sender = ctx.sender().clone();
		SpawnedSubsystem {
			name: "Sender",
			future: Box::pin(async move {
				let mut input: u64 = 0;
				for i in 0..self.0 {
					sender.send_message(Msg1029([i as u8; 1029])).await;
					input ^= (i as u8) as u64;
				}
				match ctx.recv().await.unwrap() {
					FromOrchestra::Communication { msg } => assert!(msg.0 == input),
					_ => panic!("unexpected message"),
				}
				Ok(())
			}),
		}
	}
}

#[orchestra::subsystem(Receiver, error=Yikes)]
impl<Context> ReceiverSubsys {
	fn start(self, mut ctx: Context) -> SpawnedSubsystem<Yikes> {
		let mut sender = ctx.sender().clone();
		SpawnedSubsystem {
			name: "Receiver",
			future: Box::pin(async move {
				let mut input: u64 = 0;
				for _ in 0..self.0 {
					match ctx.recv().await.unwrap() {
						FromOrchestra::Communication { msg } => input ^= msg.0[0] as u64,
						_ => panic!("unexpected message"),
					}
				}
				sender.send_message(MsgU64(input)).await;
				Ok(())
			}),
		}
	}
}

fn test_boxed_large(c: &mut Criterion) {
	let pool = ThreadPool::new().unwrap();
	c.bench_function("send boxed message 1029 bytes", |b| {
		let pool = pool.clone();
		b.iter_custom(|niters| {
			let pool = pool.clone();
			let start = executor::block_on(async move {
				let (orchestra, _handle) = BenchBoxedLarge::builder()
					.sender(SenderSubsys(niters))
					.receiver(ReceiverSubsys(niters))
					.spawner(DummySpawner(pool))
					.build()
					.unwrap();

				let start = Instant::now();
				try_join_all(orchestra.running_subsystems).await.unwrap();
				start
			});
			start.elapsed()
		})
	});
}

criterion_group!(benches, test_boxed_large);
