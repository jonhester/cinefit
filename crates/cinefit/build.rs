fn main() {
    // cinefit-render links the pinned libplacebo from its own prefix; point
    // the binary at it so the system's older copy is never loaded instead.
    println!("cargo::rerun-if-env-changed=DEP_CINEFIT_PLACEBO_LIBDIR");
    if let Ok(dir) = std::env::var("DEP_CINEFIT_PLACEBO_LIBDIR") {
        println!("cargo::rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
