# consoled-e2e

**简体中文** | [English](#english)

[`consoled`](https://github.com/BRX-Boruix/consoled) 的**端到端验收程序**——通过真实系统调用
跑完 console 字节环的完整阻塞与唤醒往返。

---

## 为什么需要它

内核自带的测试可以直接操作节点和环形缓冲，但**触及不到**两条关键路径：

1. **系统调用包装本身**——参数打包、错误码翻译、用户缓冲区的合法性校验
2. **真实的阻塞与唤醒往返**——读者在空环上挂起、写者写入后唤醒它，这需要真实的进程上下文
   切换，只能在真正的进程里验证

`consoled-e2e` 通过真实系统调用跑完整个往返，用退出码裁决成败。

## 两个角色

程序通过第一个参数选择角色：

| 角色 | 行为 |
| --- | --- |
| `writer` | 后台进程：阻塞读 console（空环时挂起）→ 收到第一段数据后逐字节校验 → 继续挂起等待第二段 → 校验 → 写 PASS 标记 → 退出 0 |
| `roundtrip` | 前台：等 writer 就绪 → 写入两段数据 → 轮询状态确认消费水位归零并与遥测对账 → 终止 writer → 收尸 → 核对 PASS 标记 |

不带参数启动时默认执行 `roundtrip`（writer 子进程由它派生）。任何失败都以非零退出。

## 测试数据为什么是两段

两段数据使用**不同的基准值**（第一段从 0x00 起，第二段从 0x80 起），且都跨 0/255 边界、带
周期性。这样既能暴露字节序或偏移错误，也能暴露**两段次序颠倒**——如果顺序错了，校验立刻失败。

## 判据

程序不只看"收到了数据"，而是与 console 设备的状态遥测**对账**：确认写进去的字节确实被消费、
消费水位归零。仅凭"读到了一些字节"无法区分"完整收到"与"只收到一部分"。

## 它在整体中的位置

这个验收程序采用**单变量实验**形态：在 `consoled` 尚未常驻的阶段，console 环的唯一写者就是
本程序自己。这恰好把"写端 → 唤醒等待者 → 读端交付"这条内核链路隔离成可判定的实验。

等到 `consoled` 接管写端之后，真实终端流由更上层的交互验收覆盖。

## 构建

```bash
cargo build --release
```

编译产物部署为 BORUIX 系统中的用户态程序后运行。

## 文件结构

```
consoled-e2e/
├── Cargo.toml    # 包定义
├── build.rs      # 注入链接脚本
├── linker.ld     # 用户态段布局
└── src/
    └── main.rs   # 两个角色的实现
```

## 相关项目

- [`consoled`](https://github.com/BRX-Boruix/consoled) —— 被验收的常驻终端字节生产者
- [`libsys`](https://github.com/BRX-Boruix/libsys) —— 用户态系统调用封装

## 许可

MIT License，版权归 Yang Borui 所有。详见 [LICENSE](LICENSE)。

---

# English

[简体中文](#consoled-e2e) | **English**

The **end-to-end acceptance program** for [`consoled`](https://github.com/BRX-Boruix/consoled) —
it drives the console byte ring through a complete block-and-wake round trip using real system calls.

---

## Why it exists

The kernel's own tests can operate nodes and ring buffers directly, but they **cannot reach** two
critical paths:

1. **The system call wrappers themselves** — argument packing, error code translation, and
   validation of user buffers
2. **A genuine block-and-wake round trip** — a reader suspending on an empty ring and being woken
   when a writer deposits data, which requires a real process context switch and can only be
   verified inside a real process

`consoled-e2e` drives that whole round trip through real system calls and lets its exit code be the
verdict.

## Two roles

The program selects its role from its first argument:

| Role | Behaviour |
| --- | --- |
| `writer` | Background process: blocks reading the console (suspending on an empty ring) → verifies the first segment byte by byte → suspends again waiting for the second → verifies it → writes a PASS marker → exits 0 |
| `roundtrip` | Foreground: waits for the writer to become ready → writes two segments → polls status to confirm the consumption level returns to zero and reconciles against telemetry → terminates the writer → reaps it → checks the PASS marker |

Started without an argument it defaults to `roundtrip` (which spawns the writer child). Any failure
exits non-zero.

## Why the test data comes in two segments

The two segments use **different base values** (the first starts at 0x00, the second at 0x80), and
both cross the 0/255 boundary with a periodic pattern. This exposes byte-order or offset errors as
well as a **swap of the two segments** — if the order is wrong, verification fails immediately.

## The criterion

The program does not merely check that "some data arrived"; it **reconciles** against the console
device's telemetry, confirming that the bytes written were actually consumed and that the
consumption level returned to zero. "I read some bytes" alone cannot distinguish a complete receipt
from a partial one.

## Where it sits in the whole

This acceptance program takes the form of a **single-variable experiment**: at the stage where
`consoled` is not yet resident, the only writer to the console ring is this program itself. That
isolates the kernel path "writer → wake the waiter → deliver to the reader" into something decidable.

Once `consoled` takes over the write side, real terminal streams are covered by higher-level
interactive acceptance tests.

## Building

```bash
cargo build --release
```

The artifact is deployed as a user-space program in a BORUIX system before running.

## Layout

```
consoled-e2e/
├── Cargo.toml    # package definition
├── build.rs      # injects the linker script
├── linker.ld     # user-space section layout
└── src/
    └── main.rs   # both roles
```

## Related projects

- [`consoled`](https://github.com/BRX-Boruix/consoled) — the resident terminal byte producer under test
- [`libsys`](https://github.com/BRX-Boruix/libsys) — the user-space syscall wrapper

## License

MIT License, copyright Yang Borui. See [LICENSE](LICENSE).
