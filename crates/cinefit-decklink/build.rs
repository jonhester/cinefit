fn main() {
    println!("cargo::rerun-if-changed=shim");
    println!("cargo::rerun-if-changed=vendor/decklink-sdk/include");

    // The SDK's dispatch layer dlopens libDeckLinkAPI.so, so this is Linux
    // only. Elsewhere the crate builds empty and the shim is never compiled.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }

    // Blackmagic's file is built as shipped, without our warning flags: its
    // warnings are not ours to fix.
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include("vendor/decklink-sdk/include")
        .file("vendor/decklink-sdk/include/DeckLinkAPIDispatch.cpp")
        .warnings(false)
        .compile("decklink_dispatch");

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include("vendor/decklink-sdk/include")
        .file("shim/shim.cpp")
        .warnings(true)
        .extra_warnings(true)
        .warnings_into_errors(true)
        .compile("decklink_shim");

    println!("cargo::rustc-link-lib=dl");
    println!("cargo::rustc-link-lib=pthread");
}
