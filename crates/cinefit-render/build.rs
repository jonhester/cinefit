fn main() {
    println!("cargo::rerun-if-changed=shim");

    // libplacebo comes from the system and cinefit renders on Linux only, so
    // elsewhere the crate builds without the shim.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }

    let placebo = pkg_config::Config::new()
        .atleast_version("7.360")
        .probe("libplacebo")
        .expect("libplacebo >= 7.360 (on Ubuntu 26.04: apt install libplacebo-dev)");
    let vulkan = pkg_config::probe_library("vulkan")
        .expect("Vulkan headers (on Ubuntu: apt install libvulkan-dev)");

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
