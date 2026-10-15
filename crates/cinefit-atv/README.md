# cinefit-atv

What an Apple TV is playing — the app, the title, where in it, and whether it
is playing — read over the remote-control channel that AirPlay 2 tunnels
(#115). It only reads: nothing it sends changes what the Apple TV does.

## Based on pyatv

This crate is a Rust reimplementation of the part of
[pyatv](https://github.com/postlund/pyatv) that reads now-playing state, by
Pierre Ståhl and its contributors, MIT-licensed. The protocol knowledge is
pyatv's: the pairing and pair-verify steps, the key-derivation salts and
labels, the AirPlay 2 `SETUP` bodies, the data-stream framing, the device
information we introduce ourselves with, and the media remote protocol's
message and field numbers (from pyatv's `.proto` files). Each module names the
pyatv files it follows.

pyatv itself is not a dependency, and is never shipped with cinefit. It is used
on the development rig as a reference: run it alongside this crate on the same
Apple TV and compare their answers.

| Module | Follows, in pyatv |
|---|---|
| `hap` | `auth/hap_srp.py`, `auth/hap_session.py`, `support/chacha20.py`, `protocols/airplay/auth/hap.py` |
| `tlv8` | `auth/hap_tlv8.py` |
| `conn` | `support/http.py`, `support/rtsp.py` |
| `stream` | `protocols/airplay/channels.py` |
| `proto` | `protocols/mrp/protobuf/*.proto`, `protocols/mrp/messages.py` |
| `lib` (`watch`, `Tracker`) | `protocols/airplay/ap2_session.py`, `protocols/airplay/__init__.py`, `protocols/mrp/protocol.py`, `protocols/mrp/player_state.py` |

## Trying it

Pair once with pyatv (`atvremote --protocol airplay pair`), then:

    CINEFIT_ATV_CREDENTIALS=<the AirPlay credentials pyatv printed> \
        cargo run -p cinefit-atv --example now_playing -- <apple-tv-host> [-v]

## pyatv's licence

Copyright (c) 2020 Pierre Ståhl

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
