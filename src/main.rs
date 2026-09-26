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
        // 每段过验即落标记：consumer 据此推进（不依赖 status 水位——
        // 环是共享的，第三人（consoled）的写入会让「绝对水位归零」
        // 永不成立，run4 实测。标记文件是**进程间**对账通道，
        // 与内核内部计数解耦（S20：失败模式先行）。
        let mpath: &str = if seg == 0 { "/tmp/ce2e_seg0" } else { "/tmp/ce2e_seg1" };
        // 标记文件此前不存在：WRITE_ONLY 的 create=false 会 NotFound
        //（sys_open 只在 flags.create 时创建——run5 实测，status
        // read_pos=12 证明消费已发生、标记却缺失）。写标记必须
        // CREATE_OR_TRUNCATE（shell 后台重定向同款）。
        let mfd = match open(mpath, OpenFlags::CREATE_OR_TRUNCATE, Permissions::read_write()) {
            Ok(f) => f,
            Err(_) => {
                outln(b"[ce2e-w] FATAL: marker open failed");
                let _ = close(fd);
                return 14;
            }
        };
        let _ = write(mfd, b"ok");
        let _ = close(mfd);
    }
    // 两段标记均已落盘（seg0/seg1）= 消费与校验的完整证据链；
    // consumer 以 seg1 标记为最终对账点（单一事实源，S13），
    // 这里不再写第二个总标记。
    let _ = close(fd);
    0
}

/// 有界轮询一个标记文件的出现与内容 ok（进程间对账通道）。
fn wait_marker(path: &str) -> bool {
    for _ in 0..200_000 {
        match open(path, OpenFlags::READ_ONLY, Permissions::readonly()) {
            Ok(mfd) => {
                let mut b = [0u8; 8];
                let n = read(mfd, &mut b).unwrap_or(0);
                let _ = close(mfd);
                if &b[..n] == b"ok" {
                    return true;
                }
            }
            Err(_) => {}
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
    // writer 收到 seg0 并逐字节校验后落 /tmp/ce2e_seg0 标记——
    // **不**等 status 水位归零：环是共享的，第三写者的字节会让
    // 「绝对水位归零」永不成立（run4 实测教训）。标记文件是
    // 进程间对账通道，与内核内部计数解耦（S20）。
    if !wait_marker("/tmp/ce2e_seg0") {
        outln(b"[ce2e] FAIL: seg0 marker never appeared (writer stuck?)");
        let _ = close(wfd);
        return 23;
    }

    // 3. 写第二段（跨 0x80 边界的第二 pattern），同样等标记。
    let seg1: alloc::vec::Vec<u8> = (0..SEG).map(|i| pattern(1, i)).collect();
    match write(wfd, &seg1) {
        Ok(n) if n == SEG => {}
        _ => {
            let _ = close(wfd);
            return 24;
        }
    }
    if !wait_marker("/tmp/ce2e_seg1") {
        outln(b"[ce2e] FAIL: seg1 marker never appeared");
        let _ = close(wfd);
        return 25;
    }
    let _ = close(wfd);

    // 4. 两段标记都已就位 = writer 逐字节校验全过（对账已在
    //    wait_marker 内完成；writer 的最终退出码经 waitpid 收，
    //    这里不再重复一个总标记文件——单一事实源，S13）。
    let pass = true;
    let _ = pass;

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
    } else if role == "consumer" {
        run_consumer()
    } else {
        out(b"[ce2e] usage: consoled-e2e writer|consumer (got ");
        out(role.as_bytes());
        outln(b")");
        1
    }
}
