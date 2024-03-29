#![allow(dead_code)]

use orchestra::*;

#[derive(Default)]
struct AwesomeSubSysA;

#[derive(Default)]
pub struct AwesomeSubSysB;

impl ::orchestra::Subsystem<OrchestraSubsystemContext<MsgA>, OrchestraError> for AwesomeSubSysA {
	fn start(self, _ctx: OrchestraSubsystemContext<MsgA>) -> SpawnedSubsystem<OrchestraError> {
		SpawnedSubsystem { name: "sub A", future: Box::pin(async move { Ok(()) }) }
	}
}
impl ::orchestra::Subsystem<OrchestraSubsystemContext<MsgB>, OrchestraError> for AwesomeSubSysB {
	fn start(self, _ctx: OrchestraSubsystemContext<MsgB>) -> SpawnedSubsystem<OrchestraError> {
		SpawnedSubsystem { name: "sub B", future: Box::pin(async move { Ok(()) }) }
	}
}

#[derive(Debug, Clone)]
pub struct DummySpawner;

impl Spawner for DummySpawner {
	fn spawn_blocking(
		&self,
		task_name: &'static str,
		subsystem_name: Option<&'static str>,
		_future: futures::future::BoxFuture<'static, ()>,
	) {
		println!("spawn blocking {} {}", task_name, subsystem_name.unwrap_or("default"))
	}

	fn spawn(
		&self,
		task_name: &'static str,
		subsystem_name: Option<&'static str>,
		_future: futures::future::BoxFuture<'static, ()>,
	) {
		println!("spawn {} {}", task_name, subsystem_name.unwrap_or("default"))
	}
}

#[derive(Clone, Debug)]
pub struct SigSigSig;

pub struct Event;

#[derive(Clone, Debug)]
pub struct MsgA(u8);

#[derive(Clone, Debug)]
pub struct MsgB(u8);

#[orchestra(signal=SigSigSig, event=Event, gen=AllMessages, error=OrchestraError, boxed_messages=true)]
pub struct Orchestra {
	#[subsystem(consumes: MsgA, sends: [MsgB])]
	sub_a: AwesomeSubSysA,

	#[subsystem(consumes: MsgB, sends: [MsgA])]
	sub_b: AwesomeSubSysB,
}

pub struct DummyCtx;

fn main() {
	let _orchestra_builder = Orchestra::builder()
		.sub_a(AwesomeSubSysA::default())
		.sub_b(AwesomeSubSysB::default())
		.spawner(DummySpawner)
		.build();
}
