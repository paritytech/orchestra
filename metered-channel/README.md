# prioritized-metered-channel

Extends the `futures` provided channels with
a metrics accessor.

Implements a metered variant of `mpsc` channels that provide an interface to extract metrics.
The following metrics are available:
- The amount of messages sent on a channel, in aggregate.
- The amount of messages received on a channel, in aggregate.
- How many times the caller blocked when sending messages on a channel.
- Time of flight in micro seconds (us)

Note: Currently there is no _prioritization_ built in! It's on the agenda, but not quite there yet.
