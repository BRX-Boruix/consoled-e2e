# consoled-e2e

BORUIX 的控制台环端到端验收程序：验证控制台字节环的阻塞唤醒与字节完整性。

[English](README.en.md)

## 测什么

两个进程协作完成一轮往返：

- `writer` 以后台方式启动，阻塞读控制台——环空时被挂起，登记为等待者
- `consumer` 在前台写两段已知字节
- 每段写入后，等待者被唤醒、逐字节核对；等待者写回通过标记后退出
- 协调端核对消费水位归零、遥测计数对账、收尸并核对退出码

它覆盖内核自检触不到的两段路径：系统调用封装本身，以及要求真实进程上下文切换的阻塞唤醒。
全程只本程序自己写环——把「写端 → 等待者唤醒 → 读端交付」隔离成单变量实验。

## 用法

命令行字为 `writer` 或 `consumer`，两端由 shell 的内建命令协调运行：

```
[ce2e] PASS (roundtrip park/wake + byte integrity)
```

## 退出码

- `0`——往返完成，字节完整，对账一致
- 非零——失败；读端打不开、字节不符、写入不完整、收尸异常各有独立编号，输出行注明

## 构建

```bash
cargo build --release
```

## 文件结构

```
consoled-e2e/
├── Cargo.toml    # 包定义
├── build.rs      # 注入链接脚本
├── linker.ld     # 用户态段布局
└── src/
    └── main.rs   # 等待端与写入端的往返实现
```

## 相关项目

- [`consoled`](https://github.com/BRX-Boruix/consoled) —— 控制台守护进程
- [`shell`](https://github.com/BRX-Boruix/shell) —— 协调两端运行

## 许可

MIT License，版权归 Yang Borui 所有。详见 [LICENSE](LICENSE)。
