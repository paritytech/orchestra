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

use futures::executor::ThreadPool;
use orchestra::*;

enum SendingMethod {
	Send,
	TrySend,
}

struct SubA {
	regular: Vec<u8>,
	priority: Vec<u8>,
	sending_method: SendingMethod,
	ready_sender: oneshot::Sender<()>,
}

pub struct SubB {
	message_limit: usize,
	result_sender: oneshot::Sender<Vec<u8>>,
	ready_receiver: oneshot::Receiver<()>,
}

impl SubA {
	fn new(
		regular: Vec<u8>,
		priority: Vec<u8>,
		sending_method: SendingMethod,
		ready_sender: oneshot::Sender<()>,
	) -> Self {
		Self { regular, priority, sending_method, ready_sender }
	}
}

impl SubB {
	fn new(
		message_limit: usize,
		result_sender: oneshot::Sender<Vec<u8>>,
		ready_receiver: oneshot::Receiver<()>,
	) -> Self {
		Self { message_limit, result_sender, ready_receiver }
	}
}

impl crate::Subsystem<OrchestraSubsystemContext<MsgA>, OrchestraError> for SubA {
	fn start(self, mut ctx: OrchestraSubsystemContext<MsgA>) -> SpawnedSubsystem<OrchestraError> {
		let mut sender = ctx.sender().clone();
		SpawnedSubsystem {
			name: "sub A",
			future: Box::pin(async move {
				for i in self.regular {
					match self.sending_method {
						SendingMethod::Send => sender.send_message(MsgB(i)).await,
						SendingMethod::TrySend => sender.try_send_message(MsgB(i)).unwrap(),
					}
				}
				for i in self.priority {
					match self.sending_method {
						SendingMethod::Send => sender.priority_send_message(MsgB(i)).await,
						SendingMethod::TrySend =>
							sender.try_priority_send_message(MsgB(i)).unwrap(),
					}
				}

				self.ready_sender.send(()).unwrap();

				Ok(())
			}),
		}
	}
}

impl crate::Subsystem<OrchestraSubsystemContext<MsgB>, OrchestraError> for SubB {
	fn start(self, mut ctx: OrchestraSubsystemContext<MsgB>) -> SpawnedSubsystem<OrchestraError> {
		SpawnedSubsystem {
			name: "sub B",
			future: Box::pin(async move {
				let mut messages = vec![];
				// Wait until sub_a sends all messages
				self.ready_receiver.await.unwrap();
				for _ in 0..self.message_limit {
					match ctx.recv().await.unwrap() {
						FromOrchestra::Communication { msg } => {
							messages.push(msg.0);
						},
						_ => panic!("unexpected message"),
					}
				}
				self.result_sender.send(messages).unwrap();

				Ok(())
			}),
		}
	}
}

#[derive(Clone, Debug)]
pub struct SigSigSig;

#[derive(Clone, Debug)]
pub struct Event;

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct MsgA(u8);

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct MsgB(u8);

#[derive(Debug, Clone)]
pub struct DummySpawner(pub ThreadPool);

impl Spawner for DummySpawner {
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

#[orchestra(signal=SigSigSig, event=Event, gen=AllMessages, error=OrchestraError, boxed_messages=true)]
pub struct Orchestra {
	#[subsystem(consumes: MsgA, sends: [MsgB], with_priority_messages)]
	sub_a: SubA,

	#[subsystem(consumes: MsgB, sends: [MsgA], with_priority_messages)]
	sub_b: SubB,
}

#[test]
fn test_priority_send_message() {
	let (result_tx, mut result_rx) = oneshot::channel::<Vec<u8>>();
	let (ready_tx, ready_rx) = oneshot::channel::<()>();
	let regular = vec![1, 2, 3];
	let priority = vec![42];
	let sub_a = SubA::new(regular.clone(), priority.clone(), SendingMethod::Send, ready_tx);
	let sub_b = SubB::new(regular.len() + priority.len(), result_tx, ready_rx);
	let pool = ThreadPool::new().unwrap();
	let (orchestra, _handle) = Orchestra::builder()
		.sub_a(sub_a)
		.sub_b(sub_b)
		.spawner(DummySpawner(pool))
		.build()
		.unwrap();

	futures::executor::block_on(async move {
		for run_subsystem in orchestra.running_subsystems {
			run_subsystem.await.unwrap();
		}
	});

	assert_eq!(
		priority.into_iter().chain(regular.into_iter()).collect::<Vec<u8>>(),
		result_rx.try_recv().unwrap().unwrap()
	);
}

#[test]
fn test_try_priority_send_message() {
	let (result_tx, mut result_rx) = oneshot::channel::<Vec<u8>>();
	let (ready_tx, ready_rx) = oneshot::channel::<()>();
	let regular = vec![1, 2, 3];
	let priority = vec![42];
	let sub_a = SubA::new(regular.clone(), priority.clone(), SendingMethod::TrySend, ready_tx);
	let sub_b = SubB::new(regular.len() + priority.len(), result_tx, ready_rx);
	let pool = ThreadPool::new().unwrap();
	let (orchestra, _handle) = Orchestra::builder()
		.sub_a(sub_a)
		.sub_b(sub_b)
		.spawner(DummySpawner(pool))
		.build()
		.unwrap();

	futures::executor::block_on(async move {
		for run_subsystem in orchestra.running_subsystems {
			run_subsystem.await.unwrap();
		}
	});

	assert_eq!(
		priority.into_iter().chain(regular.into_iter()).collect::<Vec<u8>>(),
		result_rx.try_recv().unwrap().unwrap()
	);
}
