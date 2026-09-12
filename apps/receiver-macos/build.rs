fn main() {
    println!("cargo:rerun-if-changed=native/audio.c");
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() != "macos" {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let status = std::process::Command::new("clang")
        .args([
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-c",
            "native/audio.c",
            "-o",
        ])
        .arg(out.join("audio.o"))
        .status()
        .unwrap();
    assert!(status.success(), "CoreAudio bridge compilation failed");
    assert!(
        std::process::Command::new("ar")
            .arg("rcs")
            .arg(out.join("liblan_audio.a"))
            .arg(out.join("audio.o"))
            .status()
            .unwrap()
            .success()
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=lan_audio");
    for framework in ["AudioToolbox", "CoreAudio", "CoreFoundation"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
