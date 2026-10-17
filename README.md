<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/logo/cinefit-light-accent.svg">
    <img alt="cinefit" src="docs/logo/cinefit-dark-accent.svg" width="235">
  </picture>
</p>

# cinefit

A video processor for a home theatre that runs on an ordinary Linux PC with a
Blackmagic capture card. It sits between your player and your projector:
- **Constant image height:** it finds the black bars in every shot and fills a
  scope screen at constant image height, following films that change aspect
  ratio.
- **HDR10 untouched:** it passes HDR10 through to the display's own tone
  mapping, with the HDR signalling switching as the source does.
- **Dolby Vision:** it decodes the tunnel's metadata, and handles low-latency
  Dolby Vision.
- **Like an appliance:** it starts at boot, needs no terminal, and has an
  on-screen menu, a web page and remote control.

**It starts with one theatre and grows from there.** cinefit was built for my
own room: a JVC RS3000 on a 142" scope screen, an AMD RX 6400 and a DeckLink
Mini Recorder 4K. It should work for anyone with a recent AMD GPU and a
DeckLink card who wants HDR10 passed through. NVIDIA, other capture cards,
anamorphic lenses and more come as they get tested. What works, what is
experimental and what is known not to work are in the
[setup guide](docs/setup.md).

- [Setting it up](docs/setup.md)
- [What the hardware actually does](docs/findings.md): measured behaviour of
  GPUs, capture cards, projectors and HDMI gear that differs from their
  documentation
- [For developers](docs/developers.md): the crates, several usable on their own,
  MIT or Apache-2.0

Runs on Linux with Blackmagic's Desktop Video driver. It builds and its tests
run on any platform; without the driver, it reports that it is not loaded.

## How it fits together

`cinefit play` takes every frame through the same path. The capture card
hands over the newest frame, the bar detector finds the picture inside it,
the geometry places that picture at constant image height, the renderer
crops and scales it on the GPU, and the display code flips it out at the
next vblank. The detector reads YCbCr input only: a Dolby Vision tunnel or
RGB input goes to the full frame (a crop found on one picture would trim the
next), and `detect.crop = off` skips it. Each step is
its own crate, and none of them knows about the others; the `cinefit` crate
wires them together and holds the settings.

```mermaid
flowchart LR
    source([HDMI source]) --> decklink
    subgraph play ["cinefit play: one pass per frame"]
        decklink["<b>cinefit-decklink</b><br/>capture: frame + format"] --> detect["<b>cinefit-detect</b><br/>finds the bars,<br/>picks the crop"]
        detect --> geometry["<b>cinefit-geometry</b><br/>where the picture goes"]
        geometry --> render["<b>cinefit-render</b><br/>crop, scale, color<br/>on the GPU"]
        render --> kms["<b>cinefit-kms</b><br/>mode, HDR signaling,<br/>flips"]
        decklink --> dovi["<b>cinefit-dovi</b><br/>Dolby Vision metadata<br/>from the pixels"]
        dovi -- tunneled Dolby Vision only --> render
    end
    kms --> display([Display])
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Third-party code vendored in this repository is under its own license, kept
next to it:

- Blackmagic Design DeckLink SDK headers:
  [crates/cinefit-decklink/vendor/decklink-sdk/LICENSE](crates/cinefit-decklink/vendor/decklink-sdk/LICENSE)
- The Inter typeface, for on-screen text, under the SIL Open Font License 1.1:
  [crates/cinefit/assets/fonts/OFL-Inter.txt](crates/cinefit/assets/fonts/OFL-Inter.txt)

cinefit's binary links [libplacebo](https://code.videolan.org/videolan/libplacebo)
statically, under the GNU LGPL 2.1 or later; its licence is in each release
archive. To link cinefit against a modified libplacebo, build it with
[scripts/build-libplacebo.sh](scripts/build-libplacebo.sh) (or point
`CINEFIT_LIBPLACEBO` at your own prefix) and rebuild cinefit from the source of
the same version.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
