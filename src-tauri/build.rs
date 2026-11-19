use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=native/main.swift");
    println!("cargo:rerun-if-changed=native/IdleGate.swift");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let target = env::var("TARGET").expect("Cargo target not set");
        let architecture = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("x86_64") => "x86_64",
            Ok("aarch64") => "arm64",
            _ => panic!("Unsupported macOS architecture"),
        };
        let binaries = PathBuf::from("binaries");
        fs::create_dir_all(&binaries).expect("Could not create the sidecar directory");
        let deployment_target = format!("{architecture}-apple-macosx12.0");
        let module_cache = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output not set"))
            .join("swift-module-cache");
        let status = Command::new("xcrun")
            .args(["swiftc", "-O", "-target", &deployment_target])
            .arg("-module-cache-path")
            .arg(module_cache)
            .args(["native/main.swift", "native/IdleGate.swift", "-o"])
            .arg(binaries.join(format!("activity-helper-{target}")))
            .status()
            .expect("Could not run swiftc");
        assert!(
            status.success(),
            "Could not compile the macOS activity helper"
        );
    }

    tauri_build::build();
}
