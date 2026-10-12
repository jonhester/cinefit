# Blackmagic DeckLink SDK headers

`include/` is the `Linux/include` folder of the Blackmagic Desktop Video SDK
**16.0**, unmodified. It is not covered by cinefit's MIT/Apache-2.0 license.
The license is in [LICENSE](LICENSE).

These files are © Blackmagic Design and carry their own license block, which
permits redistribution with the notice intact. Clause 0.1 of the SDK's
[End User License Agreement](https://www.blackmagicdesign.com/EULA/DeckLinkSDK)
exempts the `Linux/Include` folder from its restrictive clauses. The rest of
the SDK (samples, documentation, other platforms) is not included and must not
be added.

`DeckLinkAPIDispatch.cpp` loads `libDeckLinkAPI.so` at run time, so building
needs only these files; running needs the Desktop Video driver installed.

To upgrade, download the SDK from
<https://www.blackmagicdesign.com/desktopvideo_sdk> and replace `include/`
with its `Linux/include` folder, then update the version above.
