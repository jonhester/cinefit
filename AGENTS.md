# AGENTS.md

Working notes for coding agents: what this repository is, how to build and
check it, and the things an agent gets wrong without being told.

cinefit is a headless Linux video processor for a projector: it captures HDMI
with a Blackmagic DeckLink card, renders with libplacebo on Vulkan, and drives
the display directly through KMS, with HDR signalling under its own control.
It is developed and tested with a JVC RS3000 (a 4096x2160 panel).

## Layout

| Crate | What it is | Must not know about |
|---|---|---|
| `cinefit` | the one binary: `probe`, `capture`, `gpu`, `pattern` | — (it wires the others together) |
| `cinefit-decklink` | capture, through a C++ shim over the vendored DeckLink SDK headers | KMS, Vulkan, libplacebo |
| `cinefit-dovi` | tunneled Dolby Vision metadata from pixel parity (ETSI GS CCM 001 §6) | anything but bytes |
| `cinefit-geometry` | constant-image-height geometry: the screen window, placements, crops from measured rows | anything but arithmetic |
| `cinefit-kms` | KMS output: modesetting, flips, HDR signalling, GBM scanout buffers | libplacebo, Vulkan |
| `cinefit-render` | libplacebo on Vulkan through a C shim; holds the GPU's clocks | KMS, capture |

Keep these boundaries: pure logic stays testable anywhere, and one
implementation sits behind each interface. Do not add a vendor abstraction, a
plugin system or a second backend "for flexibility".

## Building and checking

Everything builds and its unit tests run on any platform; talking to hardware
is Linux only. On Linux, build the pinned libplacebo once first:

    scripts/build-libplacebo.sh        # into .deps/libplacebo, about 10 s

Before every push, all three must be clean:

    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

CI runs the same on `ubuntu-26.04`. When a crate starts linking a new system
library, add its `-dev` package to the CI apt line in the same commit: a
machine with everything installed will not notice it missing.

On the machine with the hardware:

    cinefit probe                      # devices; opens nothing, reads no EDID
    cinefit probe --edid               # also the input EDID: may be an upstream hotplug
    cinefit gpu                        # Vulkan, libplacebo, v210 unpack and renders read back
    cinefit capture -t 20 [--dump DIR] # what arrives on the input
    cinefit capture -t 180 --record DIR # every frame, through zstd (needs zstd)
    cinefit pattern -t 30 [--gpu] [--hdr]   # puts a picture on the display
    cinefit play --dry-run             # capture, mode, signalling, first render; display untouched
    cinefit play -t 30                 # capture to screen: puts the input on the display

`CINEFIT_DOVI_DUMPS=DIR cargo test -p cinefit-dovi --test dumps` decodes saved
frames. On the GPU against the CPU, `CINEFIT_RENDER_DUMPS=DIR cargo test -p
cinefit-render --test dumps` unpacks a dump directory in any capture format
(and renders Dolby Vision tunnel frames with their own metadata),
and `CINEFIT_V210_STILLS=DIR cargo test -p cinefit-render --test v210_stills`
a directory of compressed `v210` stills. Each is skipped when its variable is
unset.

## Hard rules

- **One program owns the capture input and one owns the display.** Another
  program running on them makes cinefit fail with a clear error; check before
  a run rather than after.
- **Anything that puts a picture on the display needs a person watching it.**
  Give the command and what to look for; do not run display tests unattended,
  and do not infer what was on screen from a log.
- **Saved frames stay out of the repository** (`--dump` writes outside the
  checkout; `/dumps/` is ignored as a backstop).
- **The vendored SDK headers stay unmodified,** and nothing else from the SDK
  is added. Upgrading means replacing the folder; see its README.
- **Never write the card's input EDID, and read it only when asked.** A write
  renegotiates with everything upstream, and taking and releasing the EDID
  interface at all may be a hotplug: the Denon in this chain then sends its
  signal with a black picture until its input is cycled (2026-10-12).
- **No root.** GPU clocks are held per process (amdgpu stable pstate), never
  through sysfs writes; do not write anything that assumes sudo.
- **libplacebo's version is pinned in `scripts/build-libplacebo.sh`.** Moving
  it means updating the commit there, `PL_API_VER` in the shim and the minimum
  in `cinefit-render/build.rs`, together.

## Conventions

- Every `unsafe` block has a `// SAFETY:` comment; clippy enforces it.
- Structs shared with a C shim are declared on both sides; assert every field
  offset and the size on both, so a change on one side fails the build.
- "Not reported" is never "no" or zero: unreported values are `None`.
- Comments explain *why*, especially where code looks wrong but is right.
  Match the density of the file you are editing.

## Workflow

- One issue, one branch, one PR whose description says `Closes #N`. The owner
  reviews and squash-merges.
- After a squash merge, rebase a stacked PR with
  `git rebase --onto origin/main <old-base> <branch>`.
- Commit messages: a short lowercase subject, then a few one-line bullets.
  No AI attribution lines in commits or PRs.
- Wait for the `check` CI job only. CodeRabbit reviews every PR; weigh its
  findings and say which you would take before applying them.
- Put measurements in the PR description: what ran, on what, with what result.

## Measuring

- A change to capture, output or rendering is not done until it has run on the
  hardware and been measured: flips missed, frames lost, values read back.
- Suspect the instrument first. When a result looks wrong, make the check show
  enough (raw bytes, which path ran) to tell its failure modes apart.
- A check that cannot fail proves nothing; prefer numerical comparisons
  (read-back, CRCs, pacing statistics) to looking at the screen.

## Private details

This repository is public. Hostnames, addresses, other people's unreleased
products and notes about the owner's setup belong in `CLAUDE.local.md` on the
machine (gitignored), never in the repository.
