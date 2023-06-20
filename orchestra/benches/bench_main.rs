use criterion::criterion_main;

mod benchmarks;

criterion_main! {
	benchmarks::bench_boxed_subsystem_u8::benches,
	benchmarks::bench_unboxed_subsystem_u8::benches,
	benchmarks::bench_boxed_subsystem_large::benches,
	benchmarks::bench_unboxed_subsystem_large::benches,
}
