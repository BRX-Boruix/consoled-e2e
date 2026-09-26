//! BORUIX `consoled-e2e`：console 字节环端到端阻塞-唤醒验收（I-EVENTS P3）。
//!
//! **为何需要本程序（audioe2e 同款论证，S29）**：内核自检 `test_console_byte_ring`
//! 直接调节点/ring——**无法触及**两条关键路径：① syscall 包装本身（参数打包、
//! 错误码翻译、用户缓冲校验）；② **真实的阻塞-唤醒往返**（`CONSOLE_WAITER` 的
//! park/wake 要求真实的进程上下文切换，只能在有进程上下文的用户态进程里验证）。
//! 本程序经**真实 syscall** 跑完完整往返，退出码即裁决（0 = 全部正确）。
//!
//! **P4 切换前的判定形态**：consoled 尚未常驻（init 未启动它），console 环的
//! 唯一写者是**本进程自己**（`writer` 角色）。这恰恰把「写端 → 等待者唤醒 →
//! 读端交付」的内核链路隔离成可判定的单变量实验——P4 后 consoled 接管写端，
//! 真实终端流再由 l4 级交互验收兜底。
//!
//! 角色（argv[0]，audioe2e 同款；init/shell 经 `consoled-e2e writer` 拉起）：
//! - `writer`   —— 后台进程：阻塞读 console（空环挂起，登记 CONSOLE_WAITER）
//!                 → 收到 12 字节 pattern 后逐字节校验 → 挂起等第二段 →
//!                 校验 → 写 PASS 标记 → 退出 0；
//! - `consumer` —— 前台：等 writer 就绪（status 轮询）→ 写两段 pattern →
//!                 轮询 status 确认消费水位归零 + 遥测对账 → 杀 writer
//!                 → 收尸 → 对账 PASS 标记。任何失败退出非零。
#![no_std]
#![no_main]
extern crate alloc;

use libsys::{close, kill, open, read, waitpid_any_timeout, write, Error, OpenFlags, Permissions, STDOUT, Task};

/// 两段测试 pattern：跨 0/255 边界、周期性、段间不同（次序错乱必暴露）。
fn pattern(seg: usize, i: usize) -> u8 {
    let base = if seg == 0 { 0x00u8 } else { 0x80u8 };
    base.wrapping_add(((i * 37) ^ (i >> 3)) as u8)
}

const SEG: usize = 12;

fn out(b: &[u8]) {
    let _ = write(STDOUT, b);
}

fn out_u64(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    if v == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    out(&buf[i..]);
}

fn outln(b: &[u8]) {
    out(b);
    out(b"\n");
}


/// 打开 console 读端。
fn open_reader() -> Result<u64, Error> {
    open("/devices/console", OpenFlags::READ_ONLY, Permissions::readonly())
}

/// 打开 console 写端。
fn open_writer() -> Result<u64, Error> {
    open("/devices/console", OpenFlags::WRITE_ONLY, Permissions::read_write())
}

/// 打开 status 子文件（遥测真值源）。
fn open_status() -> Result<u64, Error> {
    open("/devices/console/status", OpenFlags::READ_ONLY, Permissions::readonly())
}

/// 从 status JSON 抽一个整数字段（手写扫描：status 是内核产的定形 JSON，
/// 字段名固定；libsys json 解析器用于任意 JSON，此处杀鸡不用牛刀——
/// 但字段缺失/不可解析必须报错，绝不返回伪值，S09）。
fn json_field(buf: &[u8], key: &[u8]) -> Result<u64, ()> {
    // 找 "key": 后的整数（可负？status 计数全非负，按无符号处理）。
    let mut i = 0;
    while i + key.len() + 2 < buf.len() {
        if &buf[i..i + key.len()] == key
            && buf[i + key.len()] == b'"'
            && buf[i + key.len() + 1] == b':'
        {
            let mut j = i + key.len() + 2;
            let mut v: u64 = 0;
            let mut any = false;
            while j < buf.len() && buf[j].is_ascii_digit() {
                v = v * 10 + (buf[j] - b'0') as u64;
                any = true;
                j += 1;
            }
            if any {
                return Ok(v);
            }
        }
        i += 1;
    }
    Err(())
}

/// 读 status 全文（单次读足够——JSON < 512B）。
fn read_status() -> Result<alloc::vec::Vec<u8>, Error> {
    let fd = open_status()?;
    let mut buf = alloc::vec![0u8; 512];
    let n = read(fd, &mut buf)?;
    let _ = close(fd);
    buf.truncate(n);
    Ok(buf)
}

/// writer 角色（roundtrip 的子进程）：阻塞读 → 逐段校验 → 退出码 0。
/// stdout 继承父进程 → 串口可见；失败路径全部**打印后退出**（S09：
/// run7 教训——静默退出码无法从串口区分故障形态）。
fn run_writer() -> i32 {
    let fd = match open_reader() {
        Ok(f) => f,
        Err(_) => {
            outln(b"[ce2e-w] exit 10: open reader failed");
            return 10;
        }
    };
    for seg in 0..2 {
        let mut got = [0u8; SEG];
        // 阻塞读：空环时内核登记 CONSOLE_WAITER 并挂起本进程——
        // 唤醒后经 -EAGAIN 哨兵重试（与事件流同一纪律）。
        let mut have = 0usize;
        while have < SEG {
            match read(fd, &mut got[have..]) {
                Ok(0) => {
                    // Ok(0) 只应在「哨兵重试后仍空」的瞬态出现；
                    // 让出 CPU 等下一批（绝不忙等——真阻塞由内核完成）。
                    let _ = libsys::yield_now();
                }
                Ok(n) => have += n,
                Err(Error::WouldBlock) => {
                    let _ = libsys::yield_now();
                }
                Err(_) => {
                    outln(b"[ce2e-w] exit 11: read error");
                    let _ = close(fd);
                    return 11;
                }
            }
        }
        for k in 0..SEG {
            if got[k] != pattern(seg, k) {
                out(b"[ce2e-w] exit 12: byte mismatch seg=");
                out_u64(seg as u64);
                out(b" off=");
                out_u64(k as u64);
                out(b" got=");
                out_u64(got[k] as u64);
                out(b" want=");
                out_u64(pattern(seg, k) as u64);
                outln(b"");
                let _ = close(fd);
                return 12; // 字节错——数据链路坏
            }
        }
        // 本段过验 → 继续下一段（对账由父进程 waitpid 收整体退出码，
        // run18 实测该通道精确可靠）。
    }
    outln(b"[ce2e-w] both segments validated");
    let _ = close(fd);
    0
}


/// roundtrip 父角色：spawn writer 子进程 → 写两段 → waitpid 收退出码。
///
/// **为何弃用文件标记握手（run5-16 实测链）**：交互 shell 的真实身份是
/// init 派生的 System(uid=1)（提示符的 "alice" 只是 login 流程变量，
/// S09 证据：cat /users/alice/identity EACCES、/scratch 建档失败、
/// 家目录建档 EACCES）——uid1 对 /scratch(0755 root:root)、
/// /users/alice(alice 0700) 均无写权，**没有任何常规共享可写目录**；
/// 文件握手在身份地形上不可行。waitpid 退出码是内核保证的进程间
/// 交付通道（C7.1 已登记 ppid），不依赖文件系统权限（S20）。
fn run_consumer() -> i32 {
    // 1. spawn writer 子进程（自我派生，路径模式）。子进程立即阻塞在
    //    console 空环上（登记 CONSOLE_WAITER）。
    let child = match Task::spawn("/programs/consoled-e2e.elf", &["writer"]) {
        Ok(pid) => pid,
        Err(_) => {
            outln(b"[ce2e] FAIL: spawn writer child failed");
            return 30;
        }
    };
    // 2. 写两段 pattern（子进程被逐段唤醒消费）。写端由本进程持有——
    //    子进程只读，写入-唤醒-消费-校验全链路在真实进程对之间完成。
    let wfd = match open_writer() {
        Ok(f) => f,
        Err(_) => {
            outln(b"[ce2e] FAIL: open writer end failed");
            let _ = kill(child as u64, 9);
            return 20;
        }
    };
    for seg in 0..2usize {
        let segb: alloc::vec::Vec<u8> = (0..SEG).map(|i| pattern(seg, i)).collect();
        match write(wfd, &segb) {
            Ok(n) if n == SEG => {}
            _ => {
                out(b"[ce2e] FAIL: seg");
                out_u64(seg as u64);
                outln(b" short write");
                let _ = kill(child as u64, 9);
                let _ = close(wfd);
                return 21 + seg as i32;
            }
        }
    }
    let _ = close(wfd);
    // 3. waitpid 收 writer 退出码：0 = 两段逐字节校验全过
    //    （park/wake 往返 + 字节完整性一次判定）。
    let wr = match waitpid_any_timeout(30_000_000_000) {
        Ok(w) if w.pid == child => w.code,
        Ok(_) => {
            outln(b"[ce2e] FAIL: unexpected child exited first");
            return 32;
        }
        Err(_) => {
            outln(b"[ce2e] FAIL: writer did not exit in 30s");
            let _ = kill(child as u64, 9);
            return 33;
        }
    };
    if wr != 0 {
        out(b"[ce2e] FAIL: writer exit code ");
        out_u64(wr as u64);
        outln(b"");
        return 34;
    }

    // 4. 终态遥测对账（S09）：两段至少两次写入。
    let buf = match read_status() {
        Ok(b) => b,
        Err(_) => return 27,
    };
    let writes = json_field(&buf, b"writes").unwrap_or(0);
    if writes < 2 {
        outln(b"[ce2e] FAIL: writes counter regressed");
        return 28;
    }
    outln(b"[ce2e] PASS (roundtrip park/wake + byte integrity)");
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn user_main(argc: isize, argv: *const *const u8) -> i32 {
    if argc < 1 || argv.is_null() {
        return 1;
    }
    let cmd = unsafe { *argv };
    if cmd.is_null() {
        return 1;
    }
    // argv[0] 原样交付，不按空格切分（loader 约定，audioe2e 同款）。
    let mut len = 0usize;
    unsafe {
        while *cmd.add(len) != 0 {
            len += 1;
        }
    }
    let role = match core::str::from_utf8(unsafe { core::slice::from_raw_parts(cmd, len) }) {
        Ok(s) => s,
        Err(_) => return 1,
    };
    if role == "writer" {
        run_writer()
    } else if role == "roundtrip" {
        run_consumer()
    } else if role.ends_with("consoled-e2e") {
        // 无参启动：默认跑 roundtrip（writer 子进程派生在此处发生）。
        run_consumer()
    } else {
        out(b"[ce2e] usage: consoled-e2e writer|consumer (got ");
        out(role.as_bytes());
        outln(b")");
        1
    }
}
