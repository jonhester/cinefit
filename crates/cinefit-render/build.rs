use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-changed=shim");
    println!("cargo::rerun-if-env-changed=CINEFIT_LIBPLACEBO");

    // cinefit renders on Linux only; elsewhere the crate builds without the
    // shim.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }

    // The pinned libplacebo from scripts/build-libplacebo.sh, not the system
    // one. .cargo/config.toml points CINEFIT_LIBPLACEBO at its default prefix.
    let prefix = PathBuf::from(
        std::env::var("CINEFIT_LIBPLACEBO")
            .expect("CINEFIT_LIBPLACEBO: the prefix scripts/build-libplacebo.sh installed into"),
    );
    let mut pc_path: Vec<PathBuf> = [
        "lib/x86_64-linux-gnu/pkgconfig",
        "lib/aarch64-linux-gnu/pkgconfig",
        "lib/pkgconfig",
    ]
    .iter()
    .map(|d| prefix.join(d))
    .filter(|d| d.is_dir())
    .collect();
    if let Some(existing) = std::env::var_os("PKG_CONFIG_PATH") {
        pc_path.extend(std::env::split_paths(&existing));
    }
    let pc_path = std::env::join_paths(pc_path).expect("pkg-config search path");
    // SAFETY: a build script is single-threaded; nothing else reads the
    // environment while this runs.
    unsafe { std::env::set_var("PKG_CONFIG_PATH", pc_path) };

    // At least the pinned API: the system's older libplacebo fails here
    // rather than at the shim's compile-time check. Static, from the pinned
    // build's prefix: the binary carries it, and nothing at run time can load
    // another version in its place. Its own dependencies come from its .pc
    // file: the system's Vulkan loader and lcms2 as shared libraries, and
    // glslang's static archives by path.
    let archive = ["lib/x86_64-linux-gnu", "lib/aarch64-linux-gnu", "lib"]
        .iter()
        .any(|d| prefix.join(d).join("libplacebo.a").is_file());
    assert!(
        archive,
        "no libplacebo.a in {}: libplacebo is now linked statically; run scripts/build-libplacebo.sh again",
        prefix.display()
    );
    let placebo = pkg_config::Config::new()
        .atleast_version("7.375")
        .statik(true)
        .probe("libplacebo")
        .unwrap_or_else(|e| {
            panic!(
                "libplacebo >= 7.375 not found in {}: run scripts/build-libplacebo.sh ({e})",
                prefix.display()
            )
        });
    // libplacebo and glslang are partly C++, and a static archive does not
    // name its C++ runtime: link it here, or every test binary that does not
    // happen to pull it in from elsewhere fails to link.
    println!("cargo::rustc-link-lib=stdc++");
    let vulkan = pkg_config::probe_library("vulkan")
        .expect("Vulkan headers (on Ubuntu: apt install libvulkan-dev)");
    // For holding the GPU's clocks (src/power.rs).
    pkg_config::probe_library("libdrm_amdgpu")
        .expect("libdrm_amdgpu (on Ubuntu: apt install libdrm-dev)");

    cc::Build::new()
        .file("shim/placebo.c")
        .includes(&placebo.include_paths)
        .includes(&vulkan.include_paths)
        .std("c11")
        .warnings(true)
        .extra_warnings(true)
        .warnings_into_errors(true)
        .compile("placebo_shim");
}
