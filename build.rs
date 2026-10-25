#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! Use the target SDK for macOS's versioned `proc_info` C structures.
fn main() {
    println!("cargo:rerun-if-changed=src/os/socket_peer.c");
    println!("cargo:rerun-if-changed=src/os/keychain.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/os/socket_peer.c")
            .file("src/os/keychain.m")
            .compile("hive_socket_peer");
        println!("cargo:rustc-link-lib=framework=Security");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=LocalAuthentication");
    }
}
