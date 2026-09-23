fn main() {
    // libgit2-sys does not link advapi32 (token/registry/crypto APIs) on MSVC;
    // the app binary gets it transitively via Tauri, but test binaries do not.
    if cfg!(target_os = "windows") {
        println!("cargo:rustc-link-lib=advapi32");
    }
    tauri_build::build()
}
