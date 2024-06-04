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

#[derive(Clone, Copy)]
enum SendingMethod {
	Send,
	TrySend,
}

struct SubA {
	normal: Vec<u8>,
	priority: Vec<u8>,
	sending_method: SendingMethod,
	sub_b_sent_rx: oneshot::Receiver<()>,
	sub_a_sent_tx: oneshot::Sender<()>,
	sub_a_received_tx: oneshot::Sender<Vec<u8>>,
}

pub struct SubB {
	normal: Vec<u8>,
	priority: Vec<u8>,
	sending_method: SendingMethod,
	sub_a_sent_rx: oneshot::Receiver<()>,
	sub_b_sent_tx: oneshot::Sender<()>,
	sub_b_received_tx: oneshot::Sender<Vec<u8>>,
}

impl crate::Subsystem<OrchestraSubsystemContext<MsgA>, OrchestraError> for SubA {
	fn start(self, mut ctx: OrchestraSubsystemContext<MsgA>) -> SpawnedSubsystem<OrchestraError> {
		let mut sender = ctx.sender().clone();
		SpawnedSubsystem {
			name: "sub A",
			future: Box::pin(async move {
				let mut messages = vec![];
				let message_limit = self.normal.len() + self.priority.len();

				for i in self.normal {
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

				self.sub_a_sent_tx.send(()).unwrap();
				self.sub_b_sent_rx.await.unwrap();

				for _ in 0..message_limit {
					match ctx.recv().await.unwrap() {
						FromOrchestra::Communication { msg } => {
							messages.push(msg.0);
						},
						_ => panic!("unexpected message"),
					}
				}
				self.sub_a_received_tx.send(messages).unwrap();

				Ok(())
			}),
		}
	}
}

impl crate::Subsystem<OrchestraSubsystemContext<MsgB>, OrchestraError> for SubB {
	fn start(self, mut ctx: OrchestraSubsystemContext<MsgB>) -> SpawnedSubsystem<OrchestraError> {
		let mut sender = ctx.sender().clone();
		SpawnedSubsystem {
			name: "sub B",
			future: Box::pin(async move {
				let mut messages = vec![];
				let message_limit = self.normal.len() + self.priority.len();

				for i in self.normal {
					match self.sending_method {
						SendingMethod::Send => sender.send_message(MsgA(i)).await,
						SendingMethod::TrySend => sender.try_send_message(MsgA(i)).unwrap(),
					}
				}
				for i in self.priority {
					match self.sending_method {
						SendingMethod::Send => sender.priority_send_message(MsgA(i)).await,
						SendingMethod::TrySend =>
							sender.try_priority_send_message(MsgA(i)).unwrap(),
					}
				}

				self.sub_b_sent_tx.send(()).unwrap();
				self.sub_a_sent_rx.await.unwrap();

				for _ in 0..message_limit {
					match ctx.recv().await.unwrap() {
						FromOrchestra::Communication { msg } => {
							messages.push(msg.0);
						},
						_ => panic!("unexpected message"),
					}
				}
				self.sub_b_received_tx.send(messages).unwrap();

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
	#[subsystem(consumes: MsgA, sends: [MsgB])]
	sub_a: SubA,

	#[subsystem(consumes: MsgB, sends: [MsgA], can_receive_priority_messages)]
	sub_b: SubB,
}

async fn run_inner(
	normal: Vec<u8>,
	priority: Vec<u8>,
	sending_method: SendingMethod,
) -> (Vec<u8>, Vec<u8>) {
	let (sub_a_received_tx, mut sub_a_received_rx) = oneshot::channel::<Vec<u8>>();
	let (sub_b_received_tx, mut sub_b_received_rx) = oneshot::channel::<Vec<u8>>();
	let (sub_a_sent_tx, sub_a_sent_rx) = oneshot::channel::<()>();
	let (sub_b_sent_tx, sub_b_sent_rx) = oneshot::channel::<()>();

	let sub_a = SubA {
		normal: normal.clone(),
		priority: priority.clone(),
		sending_method,
		sub_b_sent_rx,
		sub_a_sent_tx,
		sub_a_received_tx,
	};
	let sub_b = SubB {
		normal: normal.clone(),
		priority: priority.clone(),
		sending_method,
		sub_a_sent_rx,
		sub_b_sent_tx,
		sub_b_received_tx,
	};
	let pool = ThreadPool::new().unwrap();
	let (orchestra, _handle) = Orchestra::builder()
		.sub_a(sub_a)
		.sub_b(sub_b)
		.spawner(DummySpawner(pool))
		.build()
		.unwrap();

	for run_subsystem in orchestra.running_subsystems {
		run_subsystem.await.unwrap();
	}

	(sub_a_received_rx.try_recv().unwrap().unwrap(), sub_b_received_rx.try_recv().unwrap().unwrap())
}

#[test]
fn test_priority_send_message() {
	let (sub_a_received, sub_b_received) =
		futures::executor::block_on(run_inner(vec![1, 2, 3], vec![42], SendingMethod::Send));

	assert_eq!(vec![1, 2, 3, 42], sub_a_received);
	assert_eq!(vec![42, 1, 2, 3], sub_b_received);
}

#[test]
fn test_try_priority_send_message() {
	let (sub_a_received, sub_b_received) =
		futures::executor::block_on(run_inner(vec![1, 2, 3], vec![42], SendingMethod::TrySend));

	assert_eq!(vec![1, 2, 3, 42], sub_a_received);
	assert_eq!(vec![42, 1, 2, 3], sub_b_received);
}
