use std::process::Command;

fn main() {
    // What `cinefit --version` and every play log say this build is: the
    // version in Cargo.toml, the last release. Only the release workflow's
    // build (CINEFIT_RELEASE=1) is that release; any other is `+dev`, that
    // release plus whatever came after. The commit and its date come from
    // git, so a build says which source it is; a `+dev` build may carry
    // uncommitted changes the hash does not show.
    println!("cargo::rerun-if-env-changed=CINEFIT_RELEASE");
    let mut version = std::env::var("CARGO_PKG_VERSION").expect("set by cargo");
    if std::env::var("CINEFIT_RELEASE").as_deref() != Ok("1") {
        version.push_str("+dev");
    }
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    // A new commit moves HEAD (or the branch it names); rebuild the line then.
    for path in ["HEAD", "packed-refs"] {
        if let Some(p) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo::rerun-if-changed={p}");
        }
    }
    if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(p) = git(&["rev-parse", "--git-path", &r])
    {
        println!("cargo::rerun-if-changed={p}");
    }
    let commit = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let date = git(&["show", "-s", "--format=%cs", "HEAD"]).unwrap_or_else(|| "unknown".into());
    println!("cargo::rustc-env=CINEFIT_VERSION_LINE={version} ({commit} {date})");
}
