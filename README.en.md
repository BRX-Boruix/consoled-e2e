# consoled-e2e

An **end-to-end acceptance program** for BORUIX, verifying that **blocking wait and wake-up round trips** on the console byte ring work in real user space.

[简体中文](README.md)

## What it tests

The program has two roles (selected by startup argument) and runs one complete console data round trip through **real syscalls**:

| Role | Behaviour |
| --- | --- |
| `writer` | Background process: blocks reading the console (suspending on an empty ring) → verifies the 12-byte segment byte by byte → suspends again for the second segment → verifies → writes a PASS marker → exits |
| `consumer` | Foreground: waits for `writer` to be ready → writes both segments → confirms the consumption watermark is back to zero and reconciles → terminates `writer` → reaps it → checks the PASS marker |

The two test segments cross the `0`/`255` boundary and differ between segments, so **an out-of-order delivery is bound to show up**.

## Why a separate program

The in-kernel self-test adjusts the node and ring buffer directly and **cannot reach** two critical paths:

1. **The syscall wrappers themselves** — argument packing, error code translation, user buffer validation;
2. **A real blocking round trip** — the waiter's suspend and wake-up require a genuine process context switch, verifiable only in a user-space process with a process context.

## The shape of the verdict

The console service daemon is not yet resident, so the **sole writer** to the console ring is this program itself (the `writer` role). That isolates the kernel chain "writing side → waiter wake-up → reading-side delivery" into a **single-variable, decidable experiment**. Once the daemon takes over the writing side, real terminal traffic is covered by higher-level interactive acceptance instead.

## Usage

The coordinating side starts both roles (`consoled-e2e writer` and so on). The exit code is the verdict.

## Exit codes

| Exit code | Meaning |
| --- | --- |
| `0` | Everything correct |
| Non-zero | Some step failed |

## Building

```bash
cargo build --release
```

## Layout

```
consoled-e2e/
├── Cargo.toml    # package definition
├── build.rs      # injects the linker script
├── linker.ld     # user-space section layout
└── src/
    └── main.rs   # writer and reader logic
```

## Related projects

- [`libsys`](https://github.com/BRX-Boruix/libsys) — provides the console syscall wrappers
- [`consoled`](https://github.com/BRX-Boruix/consoled) — the console daemon
- [`audioe2e`](https://github.com/BRX-Boruix/audioe2e) — the same argument for another domain

## License

MIT License, copyright Yang Borui. See [LICENSE](LICENSE).
