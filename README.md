# orchestra

The orchestra pattern is a partial actor pattern, with a global orchestrator regarding
relevant work items.

## proc-macro

The proc macro provides a convenience generator with a builder pattern,
where at it's core it creates and spawns a set of subsystems, which are purely
declarative.

```rust
    #[orchestra(signal=SigSigSig, event=Event, gen=AllMessages, error=OrchestraError)]
    pub struct Opera {
        #[subsystem(MsgA, sends: [MsgB])]
        sub_a: AwesomeSubSysA,

        #[cfg(any(feature = "feature1", feature = "feature2"))]
        #[subsystem(MsgB, sends: [MsgA])]
        sub_b: AwesomeSubSysB,
    }
```

* Each subsystem is annotated with `#[subsystem(_)]` where `MsgA` respectively `MsgB` are the messages
being consumed by that particular subsystem. Each of those subsystems is required to implement the subsystem
trait with the correct trait bounds. Commonly this is achieved
by using `#[subsystem]` and `#[contextbounds]` macro.
  * `#[contextbounds(Foo, error=Yikes, prefix=wherethetraitsat)]` can applied to `impl`-blocks and `fn`-blocks. It will add additional trait bounds for the generic `Context` with `Context: FooContextTrait` for `<Context as FooContextTrait>::Sender: FooSenderTrait` besides a few more. Note that `Foo` here references the name of the subsystem as declared in `#[orchestra(..)]` macro.
  * `#[subsystem(Foo, error=Yikes, prefix=wherethetraitsat)]` is a extension to the above, implementing `trait Subsystem<Context, Yikes>`.
* `error=` tells the orchestra to use the user provided
error type, if not provided a builtin one is used. Note that this is the one error type used throughout all calls, so make sure it does impl `From<E>` for all other error types `E` that are relevant to your application.
* `event=` declares an external event type, that injects certain events
into the orchestra, without participating in the subsystem pattern.
* `signal=` defines a signal type to be used for the orchestra. This is a shared "tick" or "clock" for all subsystems.
* `gen=` defines a wrapping `enum` type that is used to wrap all messages that can be consumed by _any_ subsystem.
* Features can be feature gated by `#[cfg(feature = "feature")]` attribute macro expressions. Currently supported are `any`, `all`, `not` and `feature`.

```rust
    /// Execution context, always required.
    pub struct DummyCtx;

    /// Task spawner, always required
    /// and must implement `trait orchestra::Spawner`.
    pub struct DummySpawner;

    fn main() {
        let _orchestra = Opera::builder()
            .sub_a(AwesomeSubSysA::default())
            .sub_b(AwesomeSubSysB::default())
            .spawner(DummySpawner)
            .build();
    }
```

In the shown `main`, the orchestra is created by means of a generated, compile time erroring
builder pattern.

The builder requires all subsystems, baggage fields (additional struct data) and spawner to be
set via the according setter method before `build` method could even be called. Failure to do
such an initialization will lead to a compile error. This is implemented by encoding each
builder field in a set of so called `state generics`, meaning that each field can be either
`Init<T>` or `Missing<T>`, so each setter translates a state from `Missing` to `Init` state
for the specific struct field. Therefore, if you see a compile time error that blames about
`Missing` where `Init` is expected it usually means that some subsystems or baggage fields were
not set prior to the `build` call.

To exclude subsystems from such a check, one can set `wip` attribute on some subsystem that
is not ready to be included in the `orchestra`:

```rust
    #[orchestra(signal=SigSigSig, event=Event, gen=AllMessages, error=OrchestraError)]
    pub struct Opera {
        #[subsystem(MsgA, sends: MsgB)]
        sub_a: AwesomeSubSysA,

        #[subsystem(MsgB, sends: MsgA), wip]
        sub_b: AwesomeSubSysB, // This subsystem will not be required nor allowed to be set
    }
```

Baggage fields can be initialized more than one time, however, it is not true for subsystems:
subsystems must be initialized only once (another compile time check) or be _replaced_ by
a special setter like method `replace_<subsystem>`.

A task spawner and subsystem context are required to be defined with `Spawner` and respectively `SubsystemContext` implemented.

## Debugging

As always, debugging is notoriously annoying with bugged proc-macros, see [feature `"expand"`](#feature-expand).

## Features

### feature `"expand"`

[`expander`](https://github.com/drahnr/expander) is employed to yield better
error messages. Enable with `--features=orchestra/expand`.

### feature `"dotgraph"`

Generate a directed graph which shows the connectivity according to the
declared messages to be send and consumed. Enable with `--features=orchestra/dotgraph`.
The path to the generated file will be displayed and is of the form
`${OUT_DIR}/${orchestra|lowercase}-subsystem-messaging.dot`.
Use `dot -Tpng ${OUT_DIR}/${orchestra|lowercase}-subsystem-messaging.dot > connectivity.dot` to
convert to i.e. a `png` image or use your favorite dot file viewer.
It also creates a `.svg` alongside the `.dot` graph, derived from the `.dot` graph for
direct usage.

## Caveats

No tool is without caveats, and `orchestra` is no exception.

### Large Message Types

It is not recommended to have large messages that are sent via channels, just like for other
implementations of channels.
If you need to transfer data that is larger than a few dozend bytes, use `Box<_>` around it or use a global identifier to access persisted state such as a database, depending on the use case.

### Response Channels

It seems very appealing to have response channels as part of messages, and for many cases,
these are a very convenient way of maintaining a strucutured data flow, yet they are ready
to shoot you in the foot when not used diligently. The diligence required is regarding three
topics:

1. Circular message dependencies leading to a dead-lock for single threaded subsystems
2. Too deep message dependencies across _many_ subsystems
3. Delays due to response channels

Each of them has a variety of solutions, such as local caching to remedy frequent lookups of the same information or splitting up subsystem into multiple to avoid circular dependencies or merging tiny topologically closely related to one subsystem in some exceptional cases, but strongly depend on the individual context in which orchestra is used.

To find these, the feature `dotgraph` is providing a visualization of all interactions of the subsystem to subsystem level (not on the message level, yet) to investigate cycles.
Keep an eye on warnings during the generation phase.

## License

Licensed under either of

* Apache License, Version 2.0, (LICENSE-APACHE or <http://www.apache.org/licenses/LICENSE-2.0>)
* MIT license (LICENSE-MIT or <http://opensource.org/licenses/MIT>)

at your option.
