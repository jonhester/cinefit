# Findings

What the hardware and drivers actually do, as measured while building
cinefit. Several of these contradict a UI, a manual or a header comment, and
most cost real time to find. Each says what was measured and how.

Everything here was measured on one setup (see [setup.md](setup.md)): an AMD
Radeon RX 6400 on amdgpu (kernel 7.0, Mesa 26.0), a DeckLink Mini Recorder 4K,
a JVC RS3000 (DLA-NX9) on a 142" 2.35:1 screen, and an Apple TV 4K and a
Panasonic UB820 behind a Denon receiver and an EZCOO splitter. Where a finding
may not carry over to other hardware, it says so.

**Contents**

- [NVIDIA](#nvidia)
- [DRM / KMS on amdgpu](#drm--kms-on-amdgpu)
  - [Scanout layouts](#scanout-layouts)
- [DeckLink](#decklink)
- [Pacing: two clocks, and the phase between them](#pacing-two-clocks-and-the-phase-between-them)
- [GPU clocks on amdgpu](#gpu-clocks-on-amdgpu)
- [libplacebo](#libplacebo)
  - [What its tone mapping does to film levels](#what-its-tone-mapping-does-to-film-levels)
- [The JVC's HDR tone mapping](#the-jvcs-hdr-tone-mapping)
  - [What the projector computes from MaxCLL](#what-the-projector-computes-from-maxcll)
  - [The screen's peak](#the-screens-peak)
  - [Frame Adapt breathes when shots alternate in brightness](#frame-adapt-breathes-when-shots-alternate-in-brightness)
  - [Theater Optimizer is a second input to the same curve](#theater-optimizer-is-a-second-input-to-the-same-curve)
  - [Under Frame Adapt, from a test disc](#under-frame-adapt-from-a-test-disc)
- [Low-latency Dolby Vision (LLDV)](#low-latency-dolby-vision-lldv)
- [The splitter's EDID decides what the source sends](#the-splitters-edid-decides-what-the-source-sends)
- [Real discs](#real-discs)
  - [Cuts](#cuts)
  - [A 1080p stream upscaled by the Apple TV](#a-1080p-stream-upscaled-by-the-apple-tv)
- [The Denon receiver's control interfaces](#the-denon-receivers-control-interfaces)
- [Geometry: a fixed lens on a scope screen](#geometry-a-fixed-lens-on-a-scope-screen)

## NVIDIA

cinefit started on an RTX 3060 and moved to the RX 6400 by choice (see
[setup.md](setup.md#what-you-need)). What was measured on the 3060 before the
move:

**`nvidia-drm` advertises HDR properties it cannot honour.** The connector
exposes `HDR_OUTPUT_METADATA` and `Colorspace`, and rejects every non-default
value with `EINVAL`. Bisected commit by commit:

    base only (no HDR, no Colorspace)              ACCEPTED
    Colorspace=Default                             ACCEPTED
    Colorspace=BT2020_RGB                          EINVAL
    HDR blob PQ, no Colorspace                     EINVAL
    HDR blob PQ + Colorspace=BT2020_RGB            EINVAL
    HDR blob PQ, zero luminance fields             EINVAL
    HDR blob HLG + Colorspace=BT2020_RGB           EINVAL
    HDR blob EOTF=0 (SDR) + BT2020_RGB             EINVAL
    HDR blob cleared (0) + Colorspace=Default      ACCEPTED

Tried on drivers 595.91.07 and 610.57.04, at 4096x2160 and 3840x2160, with and
without `ALLOW_MODESET`, for PQ, HLG and EOTF 0, with a correctly sized 32-byte
blob, and with `hdmi_deepcolor=1`. Only defaults are accepted. This matches
open-gpu-kernel-modules issues #779, #933 and #1285.

`nvidia-drm` also exposes neither `max bpc` nor `Broadcast RGB`, so 10-bit
output and output range cannot be requested at all. The RX 6400 exposes all of
them and accepts the HDR commit.

**Not ruled out:** whether the bisect's scanout buffer was 8-bit, so that the
driver was refusing HDR on an 8-bit plane rather than HDR outright. NVIDIA
support will start by settling that.

**Also:** NVIDIA's `-server` driver branch ships no Vulkan ICD. On headless
Ubuntu, `nvidia-headless-no-dkms-<ver>-server-open` leaves `vulkaninfo`
showing only llvmpipe, without an error. Use `nvidia-driver-<ver>-open`.

## DRM / KMS on amdgpu

- **Property names contain spaces:** `max bpc`, `content type`,
  `Broadcast RGB`. Searching for `max_bpc` makes an absent property look
  exactly like a misspelled lookup.
- **`drmModeModeInfo.vrefresh` cannot tell film rates apart.** It is an
  integer, 24 for both 23.976 and 24.000, and the JVC offers both at 4096x2160.
  The wrong one repeats a frame every ~41 s, which reads as judder and is
  miserable to track down. Compute the rate from `clock`, `htotal` and
  `vtotal`.
- **`hdr_output_metadata` has two layout traps.** `display_primaries` is an
  array of interleaved `{x, y}` pairs, not separate `x[3]` and `y[3]` arrays:
  the same size, a different meaning. And the kernel requires the blob to be
  exactly `sizeof(struct hdr_output_metadata)`, 32 bytes: 30 of payload and 2
  of padding. Both checked byte for byte against a live blob, and end to end by
  the JVC echoing the values on its info screen.
- **HDMI 2.0 bandwidth on this connector** (4:4:4, TMDS kHz; the ceiling is
  600,000):

  | mode | 8 bpc | 10 bpc |
  |---|---|---|
  | 4096x2160 @ 23.976 | 296,703 | 370,878 |
  | 4096x2160 @ 29.97 | 296,703 | 370,878 |
  | 4096x2160 @ 50 | 594,000 | 742,500 |
  | 4096x2160 @ 59.94 | 593,407 | 741,758 |

  So 4K60 is reachable at 8 bpc but not at 10. Asked for anyway, amdgpu falls
  back to 4:2:0 without saying so, and the picture just looks soft.
- **Entering or leaving HDR is a modeset; changing HDR metadata within HDR is
  not.** The driver (`amdgpu_dm_connector_atomic_check`) forces a modeset only
  when the metadata blob appears or disappears, or `Colorspace` or
  `content type` change. Its own comment: "changing the static metadata after
  it's been set is permissible". Measured with `cinefit pattern --hdr
  --max-cll 1000 --change-max-cll 200`: MaxCLL changed mid-stream, the picture
  did not blank, and the JVC re-derived its curve on the spot (see
  [below](#what-the-projector-computes-from-maxcll)). A modeset costs the JVC a
  2-6 s resync, so cinefit learns the source's format before its first commit
  and sends the mode, the colorimetry and the metadata in one atomic commit.
- **A live switch into or out of HDR does not reach the JVC; a signal it has
  to relock to does.** With the source going to HDR10 at the same 4096x2160 @
  23.976 mode, cinefit committed PQ, the metadata, `Colorspace` BT2020_RGB and
  `max bpc` 10 in one commit. `modetest -c` showed exactly that on the
  connector, and the JVC's info screen read SDR. The other direction failed the
  same way. Restarting cinefit, so the output came up from off with the same
  signalling, worked every time.
  - **The driver's side** (`amdgpu_dm.c`): when the new stream has the same
    timing as the running one, amdgpu clears `mode_changed` ("Mode change not
    required") and only rewrites the HDR InfoFrame on the running signal. The
    JVC appears to read HDR state when it locks and not to follow the
    InfoFrame afterwards. That reading fits the evidence; it was not measured
    on the wire.
  - **The fix:** into or out of HDR, cinefit commits the output inactive first,
    then lights it with the new signalling, as a player does. Checked at the
    screen: menu, HDR10 title, menu, and the JVC followed both ways, dark
    376 ms each time.
- **amdgpu sends YCbCr 4:4:4, limited range, to any HDMI display whose EDID
  lists it, and `Broadcast RGB` then does nothing.** From the driver
  (`fill_stream_properties_from_drm_display_mode`): YCbCr 4:4:4 is chosen
  whenever the sink's EDID offers it on HDMI, RGB only when it does not, and
  no connector property chooses. The colour space follows: `BT2020_RGB`
  becomes BT.2020 YCbCr limited, `Default` becomes BT.709 YCbCr.
  `Broadcast RGB` is consulted only for RGB output. So the display engine
  converts a full-range RGB framebuffer to limited-range YCbCr, and the
  display converts it back.
  - **Consequence:** the framebuffer must be full range, whatever goes out on
    the wire. Rendering limited range compresses it twice and lifts black.
  - **How it was found:** the JVC's info screen read YUV, its Input Level on
    Auto read Standard, and a PLUGE pattern was right with the JVC on limited
    and lifted on Enhanced. Two wrong explanations came first, both from
    treating what cinefit committed as a statement about the wire.
  - A 4:4:4 round trip at 10 bits costs at most about a code, and no chroma
    resolution. Getting RGB out would take an EDID override without the YCbCr
    bit, which needs root.
- **The JVC holds hot-plug asserted in standby**, so the connector stays
  `connected` with its EDID readable while the lamp is off. Waking it from
  standby did produce a hotplug (the EDID briefly unreadable), and so did some
  output starts but not others; what decides that is not known.
- **The JVC's EDID** advertises ST 2084, HLG, BT.2020 RGB and YCbCr, and
  4096x2160 at 24, 25, 30, 50 and 60 Hz. Native 4096-wide output is therefore
  available, ~14% more panel pixels than 3840 on a 2.35:1 screen.

### Scanout layouts

- **Tiled scanout is ~20% faster to render into on the RX 6400, and GBM is the
  way to get it.** `cinefit bench` renders a 3840x2160 HDR10 frame into a
  4096x2160 scanout buffer in each layout the primary plane lists
  (`IN_FORMATS`), and compares each with LINEAR after reading it back through
  GBM, which detiles. Medians: LINEAR 1.92 ms, the two AMD tiled layouts GBM
  allocates 1.53 and 1.52 ms, both bit-identical to LINEAR. Four compressed
  (DCC) layouts GBM would not allocate, and one libplacebo would not import.
  Vulkan's driver supports no layout but LINEAR for this format, and GBM also
  knows the display's placement needs, which Vulkan has no flag for.
- **Given the plane's whole list, GBM chose a layout it could not deliver
  through this path:** a compressed layout of three planes
  (`0x0200000010537b03`), which a single-plane export and import cannot carry,
  while two usable single-plane tiled layouts were in the same list. cinefit
  now takes the layout to blame out of the list and asks again, down to
  LINEAR.
- **A tiled buffer can import and still render wrong**, so cinefit renders a
  test picture into every buffer at start and checks it pixel by pixel through
  GBM's read-back, about a tenth of a second per buffer. The check can fail: a
  tiled buffer handed to the renderer labelled LINEAR came out with 8.8
  million of 8.8 million pixels wrong.

## DeckLink

The capture chain here is source, Denon receiver, splitter, DeckLink, which
matters to several of these.

- **HDR metadata over HDMI works** on the Mini Recorder 4K:
  `IDeckLinkVideoFrameMetadataExtensions` returns the EOTF, MaxCLL, MaxFALL,
  mastering luminance and primaries per frame.
- **The input jack selection is per session and reverts to SDI** when the
  process that set it exits. The revert is a hotplug on the HDMI port, and the
  Denon then stops sending until its input is cycled: it presents as "no
  signal" and looks nothing like a configuration problem.
  `IDeckLinkConfiguration::WriteConfigurationToPreferences()` makes the choice
  persist across exits and reboots. Set the connection explicitly at open
  anyway.
- **Even reading the input EDID can black out the picture upstream.** Reading
  the EDID's dynamic range through `IDeckLinkHDMIInputEDID` (only `GetInt`,
  never a write) and releasing the interface, on every capture, left the Denon
  sending a valid signal, with its format and HDR metadata, and a **black
  picture**: v210 luma exactly 64 everywhere. Every check was another capture
  that took and released the interface again, so nothing cleared it until the
  read was removed and the Denon's input was flipped once. The mechanism is
  inferred, not measured: the card restores its default EDID when the
  interface is released, which is a hotplug. Do not take that interface
  unless writing the EDID.
- **The input EDID advertises PQ but not HLG, and a write to it does not
  persist.** `bmdDeckLinkHDMIInputEDIDDynamicRange` reads `0x20000000`, the
  SDK's documented SDR|PQ default. `WriteToEDID()` is session-scoped: a second
  process right after a successful write reads the default again. Write it at
  most once, only if bits are missing, before `EnableVideoInput`, and never
  revert it on shutdown: each write is a renegotiation, and the format that
  comes back is the source's choice.
- **Request stream time in the mode's own scale**, from `GetFrameRate` (scale
  24000, duration 1001). In nanoseconds a 23.976 frame is 41,708,333.33 ticks,
  so every third delta differs by one and an equality check reports ~8
  phantom dropped frames a second.
- **The format-change callback must honour the detected pixel format, and must
  not restart unconditionally.** Enabling 10-bit YUV on a source the card
  detected as RGB 4:4:4 makes it report a format change at once; a handler
  that restarts every time spins at about twelve restarts a second and
  delivers nothing. Pick the format from `bmdDetectedVideoInputRGB444`, and
  skip the restart when the mode and format are already applied.
- **RGB 4:4:4 input happens.** A source chose it after an EDID change, and the
  splitter's presets can force it (below).
- **The EOTF is not trustworthy on the first frames after a format change.**
  A player reported SDR briefly before settling on PQ. Committing on the first
  value costs a resync to the wrong curve and a second one back. cinefit waits
  for the same EOTF on 12 frames in a row, half a second at 24p, a quarter of
  what one resync costs.
- **Discs send placeholder HDR metadata until the feature starts.** A UHD disc
  sent MaxCLL 0 ("not specified") and mastering 0 or 4000 through its warning
  screens and studio logos, and the real values (MaxCLL 181, mastering 10000)
  from the feature's first frame, ~30-45 s in. cinefit follows the metadata, so
  the real values replace the placeholders. The player also pauses its output
  for ~2 s at each segment change.
- **Per-title metadata varies a lot, and mastering luminance says least.** One
  disc is mastered at 10,000 nits with MaxCLL 181: mastering max describes the
  grading monitor, MaxCLL describes the signal. A display keying off mastering
  max would compress it as if it were fifty times brighter (see the JVC
  section for what that costs).
- **Frames keep arriving while the stream is being stopped.** The SDK may
  deliver a frame already in flight into a callback whose downstream state is
  being torn down during a restart; it crashed a film when the player changed
  mode. "The SDK calls you on its own thread" and "the SDK stops calling you
  when you ask" are different claims, and only the first is documented.
- **The card's colorimetry flag is right.** It said BT.709 for a 4K PQ stream
  because the player was sending BT.709 (see the splitter section). A
  conclusion held for an hour, that the card does not decode HDMI's extended
  colorimetry, made the picture worse. cinefit follows the card, and converts
  the source's primaries to what the connector declares.
- **The Mini Recorder 4K captures 2160p only up to 30 fps** (60 at 1080p).
  A 4K60 signal is not captured at all, so an EDID offering 4K60 can leave a
  source sending something the card cannot see.
- SDK 16.0 renamed `IDeckLinkAttributes` to `IDeckLinkProfileAttributes`. On
  Linux `REFIID` is a by-value struct.

## Pacing: two clocks, and the phase between them

Reported as judder on a slow pan, with no missed flips and no capture gaps in
the counters.

**The capture clock and the display clock are separate and unlocked.** The
card delivers at the source's 23.976, the display flips at the connector's
23.976, and nothing ties their phase. A loop that waits for vblank, takes
whatever frame is waiting, renders and flips samples the input at one instant
per period. A frame arriving just after that instant waits a whole period; the
one behind it arrives before the next sample and is dropped by latest-wins. So
the output alternates between fresh and stale frames, in a pattern set by the
drifting phase. Every frame is shown for exactly one period and none is lost
at the card, so **neither counter can see it**: both measure the output
cadence, and the fault is in which input frame each output carries.

What made it visible was counting the loop's own idle: a consumer that is
both starving and discarding is sampling at the wrong time by definition.

The fixes:
1. **Wait for a frame instead of sampling for one**, so the render starts when
   a frame exists.
2. **Render ahead of the vblank**, into a free buffer, then present the
   finished buffer at the flip. A `late` count records the vblank arriving
   with nothing ready, the one remaining way to drop a frame.

The residual is a property of the problem: a few ppm of clock difference
means one repeated or dropped frame every ~20 minutes. It is counted, not
fixed. 24p has no slack to hide it in.

## GPU clocks on amdgpu

**Under amdgpu's default clock policy, real playback ran the GPU at about half
the speed it manages back to back, and much slower on some frames.** Per-stage
timing at 2160p 23.976:

| stage | default policy, median / p95 | stable clock, median / p95 |
|---|---|---|
| GPU unpack | 6.5 ms | 3.16 ms |
| render, GPU execution | 6.0 / 21.5 ms | 3.12 / 3.14 ms |
| whole frame | 14.9 / 35.3 ms | 8.3 / 8.7 ms |

Off-screen, the shader clock sat at 500 MHz for most of a paced run under the
default policy, and the frame missed a 23.976 budget; held at a stable clock it
read 2200 MHz throughout and fit with room for EWA scaling.

- **It is the clock policy's slow ramp, not power gating.** With a manual
  performance level the graphics block still slept between frames in a
  quarter of samples, and frames were fast anyway.
- **Ruled out first, in order:** the CPU copy into staging (1.1 ms, not most
  of the cost as first believed); capturing straight into GPU-imported memory
  (zero-copy: fastest off-screen, slowest in the real pipeline, at a CPU cost
  in every GPU submission); a bulk copy into VRAM; the PCIe link speed (16 GT/s
  x4 in every sample).
- **A workload profile set through sysfs is ignored** unless
  `power_dpm_force_performance_level` is `manual` first. And on kernel 7.0 the
  driver applies the 3D_FULL_SCREEN profile itself while the graphics rings
  are busy (`amdgpu_gfx_profile_ring_begin_use`), so it was already on.
- **sysfs settings outlive the process that wrote them.** A level set for an
  experiment was still set eight hours later.
- **How cinefit holds the clock:** a stable power state on its own amdgpu
  context (`AMDGPU_CTX_OP_SET_STABLE_PSTATE`, level `standard`). It needs only
  the render node, no root, and the kernel restores the default when the
  context is freed, which closing the file does on any exit, `kill -9`
  included. A second holder is refused with `EBUSY` without disturbing the
  first.
- **The cost:** about 18 W more while held (29 W against 11 W, measured over
  ~25 s runs, before temperature settled).
- **The v210 unpack is bound by the PCIe link, whichever engine crosses it.**
  The RX 6400 is PCIe 4.0 x4: the shader reading host memory (3.16 ms) and the
  SDMA engine copying (3.14 ms) both move 22 MB at ~7 GB/s, the link's ceiling.
  Copying into mapped VRAM with the CPU is slower (~5.7 GB/s write-combined).
  A card with an x16 link raises the ceiling about fourfold.

## libplacebo

- **A texture wrapped with `pl_vulkan_wrap` starts out held** by the caller,
  until `pl_vulkan_release_ex`; one from `pl_tex_create` starts unheld. Getting
  the initial state wrong trips an assertion inside libplacebo
  (`!tex_vk->held`) rather than returning an error. A test harness built from
  `pl_tex_create` textures could never have caught it: they start in the
  opposite state.
- **Sharing the VkDevice means locking the queue:** every submission from
  outside libplacebo must hold `lock_queue`, or it races libplacebo's own.
- **An unset `plane_upscaler` borrows the main upscaler**, and EWA chroma then
  compiles a new shader at each new anisotropic scale: 46 ms for the first
  frame at each ratio. Lanczos chroma under EWA luma removes the spike. An EWA
  *downscaler* compiles at every crop change, 67 ms each.
- **A hand-built `pl_frame` is centre-sited.** `shift_x` 0 means
  `PL_CHROMA_CENTER`; HDMI 4:2:2 is co-sited, a shift of -0.5. The "unknown
  defaults to left" note in `colorspace.h` applies only to
  `pl_chroma_location_offset`. Left at 0, every frame is a quarter of a chroma
  sample off: colour only, no edge moves in luma, so nothing looks wrong.
  Measured on a ramp: 1.99 codes from the co-sited prediction, 0.01 when set.
- **Limited-range output is an exact identity; full-range output clips
  below-black and above-white.** That is arithmetic, not a choice, but it means
  a PLUGE pattern's below-black bars cannot show in full-range output, and
  detail above code 940 is lost.
- **Nothing clamps around the scaler**: below-black and above-white input keep
  their codes through a 2x upscale, and ringing below a dark edge reaches code
  19 in the output.
- **Ringing at a scope upscale** (a hard 200 to 800 edge, scaled 1.067x),
  overshoot / undershoot in codes: lanczos 12 / 12, spline36 10 / 10,
  ewa_lanczos 17 / 18, ewa_lanczossharp 13 / 14. EWA is rounder on diagonals
  but rings more on a straight edge. Anti-ringing at full strength takes every
  one to within a code.
- **A new aspect ratio is not a new shader; a new kind of scale is.** The first
  render of a 16:9 frame (a downscale onto a scope screen) took 96 ms, then
  2.2; the first scope crop (an upscale) 38 ms, then 5.3. Every further crop of
  the same kind cost the same as a repeat. So the stalls come one per kind of
  frame (scale direction, dynamic range, tone mapping), and rendering each kind
  once at start, with libplacebo's shader cache on disk, removes them.

### What its tone mapping does to film levels

From libplacebo v7.360.0's source, with its curves run on the CPU as cinefit
sets them up:

- **MaxCLL is never read for tone mapping.** The source's peak is the
  mastering display's, or HDR10+, or a measurement of the frame. Without scene
  measurement, a 10,000-nit master whose content peaks at 181 nits maps as a
  10,000-nit title.
- **The mapping is absolute.** Into a 76-nit SDR target, a 100-nit face from a
  1000-nit master comes out at 31.8 nits (spline), 43.0 (bt2390, knee 1.0),
  56.3 (bt2390, knee 0.5), 43.0 (st2094-40).
- **Spline and st2094-40 pull the scene's average toward its place in the
  screen's range** (`knee_adaptation`, 0.4 by default). On a dim screen that
  shows bright scenes darker with scene measurement than without it. Dark
  scenes are never lifted.
- **The curve acts on IPT intensity only**, and chroma is scaled after it (to
  0.83-0.93 at 100-203 nits). Gamut mapping BT.2020 to BT.2020 is a no-op, so
  nothing soft-clips; per-channel clipping is all there is.

## The JVC's HDR tone mapping

Measured with a PQ clipping ladder, varying only the declared metadata, iris
fixed, Theater Optimizer off:

| mode | MaxCLL | MaxFALL | highest step resolved |
|---|---|---|---|
| HDR (Auto) / Frame Adapt | 1000 | 400 | 400 nits |
| HDR (Auto) / Frame Adapt | 1000 | 100 | unchanged |
| HDR (PQ), fixed curve | 1000 | 400 | 600 nits |
| HDR (PQ), fixed curve | 100 | 50 | unchanged |
| HDR (Auto) / Frame Adapt | 100 | 50 | **150 nits** |

1. **Frame Adapt responds to declared MaxCLL; the fixed PQ curve does not.**
   So forwarding the real MaxCLL matters, which is the case for passthrough.
2. **MaxFALL does nothing.**
3. **Mastering luminance was not varied**, so whether it also drives the curve
   is untested.
4. **None of this is the physical peak:** Frame Adapt always maps, so the
   ladder reads where its curve flattens, not where the light runs out.

### What the projector computes from MaxCLL

In HDR (Auto), Picture Tone, Dark Level and Bright Level are a live readout of
the curve the projector derived, not settings (in HDR (PQ) they are user
values, which is why that mode ignores the metadata):

| declared MaxCLL | Picture Tone | Dark | Bright |
|---|---|---|---|
| 100 | 7 | 0 | 6 |
| 203 | 6 | 0 | 6 |
| 637 | 3 | 0 | 4 |
| 1000 | 1 | 0 | 1 |

Higher MaxCLL gives harder compression, monotonically and more steeply as it
rises. Picture Tone runs 1-7, so **MaxCLL from 100 to 1000 walks the whole
range**: it is essentially the whole knob, and MaxCLL 1000 parks the projector
at maximum compression. MaxCLL should describe the peak of the signal actually
sent. Declared too low, highlights clip; too high, they go dim and flat.

- **It follows a change mid-stream, without a resync.** MaxCLL 1000 to 200,
  MaxFALL held at 400: Picture Tone 1 to 5 and Bright Level 1 to 7 within
  seconds, no blank. So a per-scene MaxCLL can steer the curve shot by shot.
- **Where the readout is:** Picture Mode HDR10, HDR Processing **Static**,
  then Tone Mapping (HDR(Auto)) and Enter. It shows Picture Tone, Dark Level,
  Bright Level and the MaxCLL / MaxFALL received. With HDR Processing Scene by
  Scene, as in Frame Adapt HDR, the entry is not there.
- **The info screen echoes MaxCLL and MaxFALL accurately**, which makes it an
  end-to-end check of the whole HDR signalling path.

### The screen's peak

**76 nits full white**, in SDR, high lamp, BT.2020 colour profile, on the
142" screen, measured with a SpyderX.

- The meter has no correction for a lamp's spectrum, which can be ~10% in
  luminance. Ratios between readings with the same meter are reliable.
- **Placement moves the colour as well as the brightness**: keep the meter in
  one spot for a session, and keep the projector's menu out of view. Light
  from an open door changed a reading.
- **A white browser page is not a known white**: measure a known pattern
  played through the same path films take.
- The HDR mode's own peak is not measured.

### Frame Adapt breathes when shots alternate in brightness

In a dialogue scene cutting between bright sky and faces, the picture appeared
to settle brighter or darker after each cut, with Frame Adapt in its
frame-by-frame mode; its scene mode was better. The source explains why this
scene: the brightest 1% of each shot swings 2-3x at nearly every cut, every
1-2.5 s, while the HDR metadata is constant. A curve that re-fits per frame
but eases between fits spends short shots visibly converging. Whether it is
stronger through a processor than from a player straight to the projector is
not yet compared.

### Theater Optimizer is a second input to the same curve

It takes screen size and gain, computes the screen's luminance, and moves the
same Picture Tone / Dark / Bright readout. So **any test that varies metadata
with it on measures the sum of the two**; leave it off to characterise.

- **Turning it on made the picture darker, not brighter**: Off to Low was a
  visible drop. A first run through cinefit looked washed out and flat, and
  this was the cause. It is the display setting most likely to be blamed on
  the thing being tested.
- **Screen size wants the projected image, not the screen.** With a 1.896:1
  raster overshooting a 2.35:1 screen vertically, the lit area of a 142" screen
  is 147.8" diagonal; the control takes 10" steps, so 150.
- Gain feeds straight into its target, and without a meter it is a guess.

### Under Frame Adapt, from a test disc

Spears & Munsil UHD HDR Benchmark, HDR10, 1000-nit master, passed through
untouched; Frame Adapt HDR, Scene by Scene:

- **Small highlights survive almost to the master's peak**: clipping steps stay
  separate to code 719-720, about 955-960 of 1000 nits.
- **It does not track ST 2084 even at low levels**: in the disc's 62.5-500
  cd/m² tracking boxes the inner bar shows in every box, where a display
  following the curve shows solid grey. That is dynamic tone mapping by design.
- **Bright full frames blow out**: a near-white snow scene goes flat white. The
  problem is high average level, not peak.

## Low-latency Dolby Vision (LLDV)

LLDV is Dolby Vision decoded and display-mapped by the player, to a target the
display's EDID declares. On this chain it arrives as **YCbCr 4:2:2, flagged
SDR, BT.2020 colorimetry, with no HDR InfoFrame**. The pixels are PQ in
limited range. So BT.2020 without an HDR EOTF identifies LLDV here, and
`input.lldv` makes cinefit take it as PQ and pass it on as HDR10.

- **96 nits is the floor of the format.** The v2 Dolby Vision block that LLDV
  uses stores the target peak as a 5-bit step, PQ code 2055 + 65n of 4095:
  about 96, 113, 132, 155, 181, 211 nits, up to 934 and 1082 either side of
  1000. Any device advertising LLDV is bound to these steps.
- **The UB820 honours the target.** With a 96-nit block, its LLDV peaked at
  94 nits over the whole Spears & Munsil montage (a hard ceiling at PQ 0.504),
  where the same shots read 200-1000 nits at a 1082 target. cinefit then
  commits MaxCLL 96, which matches.
- **The Apple TV draws its menus outside the mapping**, at code 571 (~199 nits,
  the BT.2408 graphics white), whatever the target.
- **A frame at a renegotiation read code 1019** (10,000 nits): one frame at the
  Apple TV's 23.98 to 24 Hz switch, not picture. A brightest-level check must
  need more than one sample.
- **SDR in a BT.2020 container looks exactly like LLDV.** A disc's SDR BT.2020
  version arrives as BT.2020 with no HDR flag, so `input.lldv` takes it for
  PQ. Turn it off for such a source.
- **The player maps into the primaries the Dolby block declares**, while the
  AVI InfoFrame still says BT.2020. A block declaring P3 makes a P3 picture
  arrive labelled BT.2020, slightly oversaturated if decoded as labelled.
  Inferred, not measured.

## The splitter's EDID decides what the source sends

An HDMI splitter that strips HDCP (here an EZCOO EZ-SP12H2) sits between the
receiver and the capture card, and the sources see its EDID, not the card's.
cinefit does not handle HDCP; see [setup.md](setup.md#what-you-need). What
its EDID choices did to the signal:

- **A fixed EDID can push the source to RGB 4:4:4**, which cinefit has no
  render path for: one stock preset (4K60 4:4:4 HDR) did.
- **An EDID that advertises HDR but not BT.2020 colorimetry costs gamut in the
  player.** Behind such a preset the UB820's own info display read disc
  `BT.2020 YCbCr 4:2:0`, HDMI out `HDR(PQ)/BT.709 YCbCr 4:2:2`. Colours outside
  BT.709 are gone before anything downstream sees them, and a processor that
  declares the result BT.2020 without converting stretches BT.709 across
  BT.2020: a baseball infield turned saturated red.
- **Copy mode, which passes the capture card's EDID up, gets BT.2020 4:2:2 at
  23.976.** The player still flags its menus BT.709 inside PQ, so a per-frame
  colorimetry conversion stays useful. The cost: every change to the card's
  EDID reaches the player as a hotplug.
- **Players send full Dolby Vision to a TV-led Dolby Vision EDID**, as the RGB
  8-bit tunnel the card captures whole, **and LLDV to an LLDV-only one.** Its
  block's peak sets the LLDV target (above).
- **An EDID offering 4K60 can leave an Apple TV sending what the card cannot
  capture** (see DeckLink); a 24 or 30 Hz Format on the Apple TV fixes it.
- **The receiver is in the path too**: the players read the Denon's EDID,
  which it builds from the splitter's.
- **A fixed EDID did not stop the receiver needing an input flip** after the
  card's EDID changed. A check that watched for "signal detected" first called
  it solved; the signal was there, and black. A check for picture must look at
  pixels.

## Real discs

- **Letterbox bars are flat, but the rows next to the picture are not.** On
  one UHD disc, 99% of bar samples are exactly code 64 and 99.99% within
  64-67, but compression ringing spreads a few rows out from the picture: rows
  1-6 outside it have 0.2-3.9% of samples above bar + 4, and under 0.1% above
  bar + 16. A margin set from the flat interior counts ringing as picture; it
  has to come from the edge profile.
- **Compression rings in the chroma too, and ringing and dark coloured picture
  overlap in size.** Near-black samples next to the picture reach 31 codes
  from neutral chroma. Raising the chroma threshold to clear it turns dark
  full-height picture into bars. They differ in shape instead: ringing lights
  isolated chroma pairs, picture lights stretches of a row.
- **Dark picture near a frame edge reads as bar.** A fade or a dark full-height
  shot can look like a one-sided or asymmetric letterbox. The edge has to be
  measured as a run of lit rows, not the first one.
- **Authored cards have lines on the outermost rows.** A rating card had
  uniform one-row lines on rows 0 and 2159 with black inside them; a
  first-lit-row edge test read the card as full height.
- **Scope is not always 2.39.** Measured heights: 1,600 rows (2.400) and 1,601
  (2.399) on two UHD discs. Two 2.39-ish ratios are only a few rows apart, so
  sections are better named by measured height than by ratio.
- **Lossless capture is large and grain decides how large**: ~8 to ~19 GB a
  minute of v210 through zstd, the grainiest film close to incompressible.

### Cuts

- **Fast motion through busy detail passes per-frame cut thresholds on every
  frame; a spike against the previous frame tells cuts from it.** Over 29,527
  frames, thresholds on mean cell difference, cells changed and histogram
  flagged 354 frames: labelled by eye once, 175 real cuts, 160 false (a camera
  rushing through corn, launch smoke, animated logos), 17 ambiguous. Before a
  real cut the change is calm (median 8.6 codes); inside motion it is not
  (median 65). Requiring the mean to be at least **twice the previous frame's**
  kept 171 of 175 real cuts and 26 of 160 false. Waiting a frame to confirm
  cost a frame of latency and lost more real cuts for a similar false count.
- **Cuts between dark shots fall under the thresholds**, and lowering them
  adds more false cuts than real ones. A missed cut is harmless.
- **2x2 points per grid cell is enough**; one point lets grain through.

### A 1080p stream upscaled by the Apple TV

Bar edges, measured as the averaged luma profile across the edge (10-90%
rise, and overshoot inside the picture):

| source | rise | overshoot |
|---|---|---|
| UHD disc, digital film | 0.84 rows | none |
| UHD disc, film scan | 2.09 rows | none |
| 1080p stream, Apple TV upscale | 1.53 rows | +9% |

The Apple TV upscales with a sharpening filter: the fingerprint is the
overshoot, not a wider edge. With heavy compression noise in dark picture on
top of it, the measured bar wobbles by several rows from frame to frame, which
a crop policy has to tolerate. cinefit sees the frame before any scaling of
its own, so none of this is cinefit's.

## The Denon receiver's control interfaces

cinefit reads the receiver's power and input (`avr.denon`) and never changes
anything on it. Two interfaces, on a Denon X3700H:

- **The web status** (`GET :8080/goform/formMainZone_MainZoneXmlStatusLite.xml`)
  gives power and the current input, holds no connection, and is what cinefit
  polls.
- **The text protocol on TCP port 23**, plain ASCII, no authentication, gives
  the names set on the receiver for its inputs (`SSFUN ?`). Four things that
  each cost a debugging round:
  1. **Replies end in CR, not LF.** A reader splitting on newlines blocks
     forever.
  2. **One session at a time.** A second connection is refused while one is
     open, so a command can work while a separate connection checking it
     reads nothing and reports a failure that did not happen. Open, ask,
     close; never hold it.
  3. **Commands need a gap.** Back-to-back writes are dropped; ~0.4 s is
     enough.
  4. **Replies are asynchronous broadcasts.** Changing the input emits several
     unsolicited lines (`SV`, `SD`, `PW`...), so a reader must skip whatever
     does not start with the prefix it asked for.

**Cycling the receiver's input is a recovery that destroys what it
recovers.** Switching away and back drops HDMI, which reliably forces a
renegotiation, and an Apple TV stops playing when it loses HDMI. Do it only
when recovery is actually needed, never by routine before a capture.

## Geometry: a fixed lens on a scope screen

cinefit drives the JVC RS3000 (DLA-NX9) at its native 17:9 raster,
4096x2160. It accepts 16:9 (3840x2160) too, but 17:9 puts ~14% more panel
pixels on a 2.35:1 screen (4096x1743 against 3840x1634). The raster is
1.896:1 and the screen 2.35:1, so a fixed lens can match one dimension.
Matching *height* would cap every picture at 1.896:1, so the lens matches the
raster's **width** to the screen's width and the raster overshoots vertically
onto the masking.

**Screen size cancels out; only the aspect ratio matters:**

    usable rows   = raster width / screen ratio = 4096 / 2.35 = 1743
    screen window = { x: 0, y: 209, w: 4096, h: 1742 }

About 19% of the panel's rows carry black that lands off the screen, so ~19%
of the light is thrown away. That is the real case for an anamorphic lens:
**light, not detail.** Scope content arrives as 3840x1634 and is shown on
4096x1742, a 1.067x upscale; an anamorphic lens would put the same 1,634
source rows on 2,160 panel rows, more lit pixels but no more information.

So **the downscaler matters more than the upscaler**, the reverse of the usual
advice. A 1.067x upscale hardly depends on the filter; 16:9 content is a 0.807x
*downscale* (3840x2160 to 3097x1742), and that is where aliasing on text
shows.

**17:9 or 16:9 is still an open choice.** At 17:9 every picture is scaled,
scope included. At 16:9 a scope picture could go to the panel 1:1, filling the
screen's width without scaling. Only a picture of exactly 2.35:1 would fill
its height too: a 2.39 or 2.40 one shown 1:1 leaves a thin bar above and
below, and filling it means scaling anyway. Narrower pictures are scaled
either way. 16:9 would also give up the ~14% more pixels, so 17:9 is the
choice for now.
