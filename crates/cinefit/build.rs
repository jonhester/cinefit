use std::process::Command;

fn main() {
    // What `cinefit --version` and every play log say this build is. The
    // release workflow sets CINEFIT_VERSION from the tag (a release, a
    // release candidate or a nightly, tools/version.sh); any other build is
    // `0.0.0-dev`. The commit and its date come from git, so a build says
    // which source it is; "-dev" builds may carry uncommitted changes the
    // hash does not show.
    println!("cargo::rerun-if-env-changed=CINEFIT_VERSION");
    let version = std::env::var("CINEFIT_VERSION").unwrap_or_else(|_| "0.0.0-dev".into());
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
