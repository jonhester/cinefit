# For developers

cinefit is MIT or Apache-2.0, your choice. Use any of it in your own project,
open or not. A credit is appreciated, and so are fixes sent back.

cinefit started in C++, in a private repository, on 4 October 2026. A week
later, on 11 October, I started again in Rust, in this repository, MIT or
Apache-2.0 from its first commit, for no deeper reason than that I wanted to.
What I did want was code that is modular and easy to test, which is
why it is split into crates with narrow jobs. C bindings for any of them
should still be straightforward, for a project in another language.

Most of the crates know nothing about the others, and several need no
hardware at all. They are the easiest to take on their own:

| Crate | What it does | Needs |
|---|---|---|
| `cinefit-dovi` | Dolby Vision tunneled over HDMI: the display-management metadata a source hides in the pixels (ETSI GS CCM 001 clause 6), turned into packets and then metadata: the grade's range, level 1 (shot brightness), level 2 (trims per target), level 4, level 5 | bytes only |
| `cinefit-detect` | the bar detector and the crop policy: rows of luma and chroma in, the crop out; reads v210 and the Dolby Vision tunnel in place | bytes only |
| `cinefit-geometry` | constant image height: where the picture goes on the screen, and the crop from measured rows | arithmetic only |
| `cinefit-atv` | what an Apple TV is playing (title, app, position) over its AirPlay remote-control channel, read-only, after [pyatv](https://github.com/postlund/pyatv) | the network |
| `cinefit-kms` | KMS output: modesetting, page flips, HDR signalling, scanout buffers | Linux, libdrm, GBM |
| `cinefit-render` | libplacebo on Vulkan through a small C shim: unpacking capture formats and rendering | Vulkan, libplacebo |
| `cinefit-decklink` | DeckLink capture through a C++ shim | Desktop Video |
| `cinefit-input` | the remote and keyboards from `/dev/input` | Linux |
| `cinefit` | the binary: wires the others together, settings, OSD, web page | all of the above |

Building and the checks every change must pass are in
[AGENTS.md](../AGENTS.md):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Everything builds, and its unit tests run, on any platform. Talking to the
hardware is Linux only.

What was learned the hard way about the hardware and drivers (things that
behave differently from their documentation) is in [findings](findings.md).
That is probably the most useful part for anyone building something similar.
