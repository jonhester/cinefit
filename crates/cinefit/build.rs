fn main() {
    // cinefit-render links the pinned libplacebo from its own prefix; point
    // the binary at it so the system's older copy is never loaded instead.
    // $ORIGIN comes first: an installed copy (tools/install.sh, release
    // archives) carries the library beside itself, and must not depend on a
    // build tree that may have changed or gone. Nothing sits beside the binary
    // in the build tree, so there the prefix after it is what loads.
    println!("cargo::rerun-if-env-changed=DEP_CINEFIT_PLACEBO_LIBDIR");
    if let Ok(dir) = std::env::var("DEP_CINEFIT_PLACEBO_LIBDIR") {
        println!("cargo::rustc-link-arg=-Wl,-rpath,$ORIGIN");
        println!("cargo::rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
