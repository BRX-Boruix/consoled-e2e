//! 用户态程序链接脚本注入（audiod/evsrcdemo 同款，S28）。

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "none" {
        let dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        println!("cargo:rustc-link-arg=-T{}/linker.ld", dir);
        println!("cargo:rustc-link-arg=-no-pie");
    }
}
