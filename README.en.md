# consoled-e2e

A BORUIX console ring end-to-end acceptance test: blocking wake-up and byte integrity of the console byte ring.

[简体中文](README.md)

## What it tests

Two processes complete one round trip:

- `writer` starts in the background and blocks reading the console — with an empty ring it is suspended and registered as a waiter
- `consumer`, in the foreground, writes two segments of known bytes
- After each write the waiter wakes, verifies the bytes; the waiter writes back a pass marker and exits
- The coordinator checks the consumption watermark returned to zero, reconciles the telemetry counters, reaps the writer and checks its exit code

It covers two paths kernel self-tests cannot reach: the syscall wrappers themselves, and the blocking
wake that requires a real process context switch. Nothing else writes the ring during the run,
isolating "writer → waiter wake → reader delivery" as a single-variable experiment.

## Usage

The command word is `writer` or `consumer`; the two sides are coordinated by a shell builtin:

```
[ce2e] PASS (roundtrip park/wake + byte integrity)
```

## Exit codes

- `0` — round trip completed, bytes intact, telemetry reconciled
- non-zero — failure; reader failed to open, byte mismatch, short write and reaping anomalies each carry their own number, named in the output

## Building

```bash
cargo build --release
```

## Repository layout

```
consoled-e2e/
├── Cargo.toml    # package manifest
├── build.rs      # injects the linker script
├── linker.ld     # user-space segment layout
└── src/
    └── main.rs   # the waiter and writer round trip
```

## Related projects

- [`consoled`](https://github.com/BRX-Boruix/consoled) — the console daemon
- [`shell`](https://github.com/BRX-Boruix/shell) — coordinates the two sides

## License

MIT License, copyright Yang Borui. See [LICENSE](LICENSE).
