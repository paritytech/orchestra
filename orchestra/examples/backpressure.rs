// Copyright (C) 2022 Parity Technologies (UK) Ltd.
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

//! This is an example of how backpressure between subsystems may be created using the
//! `recv_signal()` method.
//!
//! Subsystems A and B receive a heartbeat signal every second. Once a signal is received, subsystem
//! B sends a message to subsystem A asking the latter to do some job. The job takes 5 to 30
//! seconds, so subsystem A cannot keep up as it can only process five tasks concurrently.
//!
//! When subsystem A is saturated with tasks, it stops receiving new messages but keeps receiving
//! orchestra signals. At that moment, subsystem B is blocked in `send_message()` and cannot
//! process either signals or messages as it doesn't implement separate `recv_signal()` logic.
//!
//! Once a task is complete, subsystem A is ready to accept a new message again. Subsystem B
//! unblocks and continues its loop, processing new signals and saturating subsystem A with new
//! tasks again.
//!
//! Please note that the orchestra message capacity has been chosen to be equal to subsystem A task
//! capacity to demonstrate the backpressure logic better. The message capacity may (and probably
//! should) be larger than the task capacity to create additional buffering, which allows not
//! blocking too early.
//!
//! At the same time, signal capacity has been chosen to be superfluous. That's because subsystem B,
//! being intentionally implemented suboptimally in this example, fails to consume signals when it
//! is stuck because subsystem A backpressures on it. If the signal capacity is too low, it may
//! lead to signal channel capacity overflow, and subsystem A would stop receiving signals as well.
//! That demonstrates that in a well-designed backpressure system, all of its parts
//! (subsystems) must be able to handle backpressure gracefully; otherwise, all the actors may
//! experience signal starvation.

use std::sync::atomic::{AtomicUsize, Ordering};

use futures::{executor::ThreadPool, pin_mut};
use futures_time::{task::sleep, time};
use orchestra::*;
use rand::prelude::*;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone)]
pub enum Signal {
	Heartbeat,
}

#[derive(thiserror::Error, Debug)]
enum MyError {
	#[error(transparent)]
	Generated(#[from] OrchestraError),
}

#[derive(Debug)]
pub struct Message1(usize);
#[derive(Debug)]
pub struct Message2;
struct DummyEvent;

#[derive(Debug, Clone)]
pub struct MySpawner(pub ThreadPool);

impl Spawner for MySpawner {
	fn spawn_blocking(
		&self,
		_task_name: &'static str,
		_subsystem_name: Option<&'static str>,
		future: futures::future::BoxFuture<'static, ()>,
	) {
		self.0.spawn_ok(future);
	}

	fn spawn(
		&self,
		_task_name: &'static str,
		_subsystem_name: Option<&'static str>,
		future: futures::future::BoxFuture<'static, ()>,
	) {
		self.0.spawn_ok(future);
	}
}

struct SubsystemA;
struct SubsystemB;

#[orchestra(signal=Signal, event=DummyEvent, gen=AllMessages, error=OrchestraError, message_capacity=5, signal_capacity=600)]
struct MyOrchestra {
	#[subsystem(Message1, sends: [Message2])]
	sub_a: SubsystemA,
	#[subsystem(Message2, sends: [Message1])]
	sub_b: SubsystemB,
}

#[orchestra::subsystem(SubsystemA, error=OrchestraError)]
impl<Context> SubsystemA {
	fn start(self, mut ctx: Context) -> SpawnedSubsystem<OrchestraError> {
		SpawnedSubsystem {
			name: "SubsystemA",
			future: Box::pin(async move {
				const TASK_LIMIT: usize = 5;
				let mut tasks = FuturesUnordered::new();
				'outer: loop {
					loop {
						select! {
							from_orchestra = ctx.recv().fuse() => {
								match from_orchestra {
									Ok(FromOrchestra::Signal(sig)) => {
										log::info!(target: "subsystem::A", "received SIGNAL {sig:?} in message-processing loop");
									},
									Ok(FromOrchestra::Communication { msg }) => {
										log::info!(target: "subsystem::A", "received MESSAGE {msg:?}");
										let Message1(id) = msg;
										let dur = time::Duration::from_secs((5..30).choose(&mut rand::thread_rng()).unwrap());
										tasks.push(async move {
											log::info!(target: "task", "[{id}]: sleeping for {dur:?}");
											sleep(dur).await;
											log::info!(target: "task", "[{id}]: woke up");
											id
										});
										if tasks.len() >= TASK_LIMIT {
											break;
										}
									},
									Err(_) => {
										break 'outer;
									}
								}
							},
							id = tasks.select_next_some() => {
								log::info!(target: "subsystem::A", "task {id} finished in message-processing loop");
							}
						}
					}

					log::warn!(target: "subsystem::A", "↓↓↓ saturated, only processing signals ↓↓↓");

					loop {
						select! {
							sig = ctx.recv_signal().fuse() => {
								log::info!(target: "subsystem::A", "received SIGNAL {sig:?} in signal-processing loop");
							},
							id = tasks.select_next_some() => {
								log::info!(target: "subsystem::A", "task {id} finished in signal-processing loop");
								if tasks.len() < TASK_LIMIT {
									break;
								}
							}
						}
					}

					log::warn!(target: "subsystem::A", "↑↑↑ desaturated, processing everything ↑↑↑");
				}
				Ok(())
			}),
		}
	}
}

#[orchestra::subsystem(SubsystemB, error=OrchestraError)]
impl<Context> SubsystemB {
	fn start(self, mut ctx: Context) -> SpawnedSubsystem<OrchestraError> {
		let mut sender = ctx.sender().clone();
		SpawnedSubsystem {
			name: "SubsystemB",
			future: Box::pin(async move {
				loop {
					select! {
						from_orchestra = ctx.recv().fuse() => {
							match from_orchestra {
								Ok(FromOrchestra::Signal(sig)) => {
									let id = COUNTER.fetch_add(1, Ordering::AcqRel);
									log::info!(target: "subsystem::B", "received SIGNAL {sig:?}, sending task [{id}]");
									sender.send_message(Message1(id)).await;
									log::info!(target: "subsystem::B", "successfully sent task [{id}]");
								},
								Ok(FromOrchestra::Communication { msg }) => {
									log::info!(target: "subsystem::B", "received MESSAGE {msg:?}");
								},
								Err(_) => break
							}
						},
					}
				}
				Ok(())
			}),
		}
	}
}

fn main() {
	env_logger::builder().filter_level(log::LevelFilter::Info).init();

	let (mut orchestra, _handle) = MyOrchestra::builder()
		.sub_a(SubsystemA)
		.sub_b(SubsystemB)
		.spawner(MySpawner(ThreadPool::new().unwrap()))
		.build()
		.unwrap();

	let fut = orchestra.running_subsystems.into_future().fuse();
	pin_mut!(fut);

	let signal_spammer = async {
		loop {
			sleep(time::Duration::from_secs(1)).await;
			let _ = orchestra.sub_a.send_signal(Signal::Heartbeat).await;
			let _ = orchestra.sub_b.send_signal(Signal::Heartbeat).await;
		}
	};

	pin_mut!(signal_spammer);

	futures::executor::block_on(async {
		loop {
			select! {
				_ = signal_spammer.as_mut().fuse() => (),
				_ = fut => (),
			}
		}
	})
}
