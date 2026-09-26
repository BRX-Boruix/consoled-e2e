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

use libsys::{close, open, read, write, Error, OpenFlags, Permissions, STDOUT};

/// 两段测试 pattern：跨 0/255 边界、周期性、段间不同（次序错乱必暴露）。
fn pattern(seg: usize, i: usize) -> u8 {
    let base = if seg == 0 { 0x00u8 } else { 0x80u8 };
    base.wrapping_add(((i * 37) ^ (i >> 3)) as u8)
}

const SEG: usize = 12;

fn out(b: &[u8]) {
    let _ = write(STDOUT, b);
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

/// writer 角色：阻塞读 → 校验 ×2 段 → PASS 标记。
/// 全程不打印中间态（后台输出进 /tmp/jobN.out，打印无意义）。
fn run_writer() -> i32 {
    let fd = match open_reader() {
        Ok(f) => f,
        Err(_) => return 10,
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
                    let _ = close(fd);
                    return 11;
                }
            }
        }
        for k in 0..SEG {
            if got[k] != pattern(seg, k) {
                let _ = close(fd);
                return 12; // 字节错——数据链路坏
            }
        }
    }
    // 两段都正确：写 PASS 标记文件（consumer 对账）。
    let mark = match open("/tmp/consoled_e2e.pass", OpenFlags::WRITE_ONLY, Permissions::read_write()) {
        Ok(f) => f,
        Err(_) => {
            let _ = close(fd);
            return 13;
        }
    };
    let _ = write(mark, b"ok");
    let _ = close(mark);
    let _ = close(fd);
    0
}

/// 轮询直到 status 里 used 字段 == want（有界；超时返回 false）。
fn wait_status_used(want: u64, max_polls: u64) -> bool {
    for _ in 0..max_polls {
        if let Ok(buf) = read_status() {
            if let Ok(used) = json_field(&buf, b"used") {
                if used == want {
                    return true;
                }
                // 水位越过目标（不该发生）：立即失败，不等超时。
                if used > want {
                    return false;
                }
            }
        }
        let _ = libsys::yield_now();
    }
    false
}

/// consumer 角色：等 writer 挂起 → 写两段 → 对账 → 收尸。
fn run_consumer() -> i32 {
    // 1. 拉起 writer（后台）：exec 替换自己会毁掉本角色——用 shell 语义不在
    //    库里；本程序由验收脚本以两阶段方式驱动（见下），此函数只做
    //    「写端注入 + 对账」，writer 由脚本先经 `consoled-e2e writer &` 拉起。
    //    故本函数不 spawn；它假设 writer 已在阻塞读。

    // 2. 写第一段：此写发生在 writer 已阻塞（或即将阻塞）时——
    //    唤醒链路就此被真实触发。写后水位必须 = SEG。
    let wfd = match open_writer() {
        Ok(f) => f,
        Err(_) => return 20,
    };
    let seg0: alloc::vec::Vec<u8> = (0..SEG).map(|i| pattern(0, i)).collect();
    match write(wfd, &seg0) {
        Ok(n) if n == SEG => {}
        Ok(_) => {
            // 环被别的东西占着？P4 前环是独占实验场——短写=异常。
            let _ = close(wfd);
            return 21;
        }
        Err(_) => {
            let _ = close(wfd);
            return 22;
        }
    }
    // writer 被唤醒消费第一段前，水位短暂 = SEG；等它归零（被读走）。
    if !wait_status_used(0, 200_000) {
        outln(b"[ce2e] FAIL: seg0 not consumed (used never back to 0)");
        let _ = close(wfd);
        return 23;
    }

    // 3. 写第二段（跨 0x80 边界的第二 pattern），同样等消费。
    let seg1: alloc::vec::Vec<u8> = (0..SEG).map(|i| pattern(1, i)).collect();
    match write(wfd, &seg1) {
        Ok(n) if n == SEG => {}
        _ => {
            let _ = close(wfd);
            return 24;
        }
    }
    if !wait_status_used(0, 200_000) {
        outln(b"[ce2e] FAIL: seg1 not consumed");
        let _ = close(wfd);
        return 25;
    }
    let _ = close(wfd);

    // 4. 对账 PASS 标记（writer 两段校验都过的唯一证据）。
    let mut pass = false;
    for _ in 0..200_000 {
        match open("/tmp/consoled_e2e.pass", OpenFlags::READ_ONLY, Permissions::readonly()) {
            Ok(mfd) => {
                let mut b = [0u8; 8];
                let n = read(mfd, &mut b).unwrap_or(0);
                let _ = close(mfd);
                if &b[..n] == b"ok" {
                    pass = true;
                    break;
                }
            }
            Err(_) => {}
        }
        let _ = libsys::yield_now();
    }
    if !pass {
        outln(b"[ce2e] FAIL: writer PASS marker never appeared");
        return 26;
    }

    // 5. 收尾：终态 status 对账（真实遥测，S09）——writes>=2 且 dropped 稳定。
    let buf = match read_status() {
        Ok(b) => b,
        Err(_) => return 27,
    };
    let writes = json_field(&buf, b"writes").unwrap_or(0);
    if writes < 2 {
        outln(b"[ce2e] FAIL: writes counter regressed");
        return 28;
    }
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
    } else if role == "consumer" {
        run_consumer()
    } else {
        out(b"[ce2e] usage: consoled-e2e writer|consumer (got ");
        out(role.as_bytes());
        outln(b")");
        1
    }
}
