// Capture: the input, its SDK callback, and the restart on a format change.
// Which pixel format to capture is decided in Rust; this file only carries out
// the decision.
#include "shim.h"

#include <atomic>
#include <cmath>
#include <cstddef>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <set>

#include "DeckLinkAPI.h"

static_assert(sizeof(cf_dl_format) == 40, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_format, frame_duration) == 0, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_format, time_scale) == 8, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_format, width) == 16, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_format, height) == 20, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_format, display_mode) == 24, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_format, detected_flags) == 28, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_format, enabled_mode) == 32, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_format, enabled_pixel_format) == 36, "layout must match src/ffi.rs");

static_assert(sizeof(cf_dl_frame_info) == 160, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, stream_time) == 0, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, stream_duration) == 8, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, row_bytes) == 16, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, width) == 24, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, height) == 28, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, pixel_format) == 32, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, flags) == 36, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, colorspace) == 40, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, eotf) == 48, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, red_x) == 56, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, red_y) == 64, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, green_x) == 72, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, green_y) == 80, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, blue_x) == 88, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, blue_y) == 96, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, white_x) == 104, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, white_y) == 112, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, max_mastering) == 120, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, min_mastering) == 128, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, max_cll) == 136, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, max_fall) == 144, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_frame_info, dolby_vision_bytes) == 152, "layout must match src/ffi.rs");

namespace {

// The input opens in the mode the card already detects, read from its status
// with the sampling and bit depth, so the common case never restarts. It used
// to open in 720p50, a mode no source here sends, to force a format change
// and so learn the detected flags; that made every open restart the input
// inside the callback within its first frames, which the C++ version never
// did. With no signal detected it still opens in 720p50: the SDK reports only
// what differs from the enabled mode, so a source arriving in a guessed mode
// (the C++ version's 2160p23.98 YCbCr) would never be reported at all. The
// cost is one restart when the signal arrives, which is a change anyway.
constexpr BMDDisplayMode kFallbackMode = bmdModeHD720p50;
constexpr BMDPixelFormat kFallbackPixelFormat = bmdFormat10BitYUV;

bool same_iid(REFIID a, REFIID b) { return memcmp(&a, &b, sizeof a) == 0; }

// Reads the frame's metadata into info. Everything starts unreported, so a
// value the card does not give reads as absent rather than as zero.
void read_metadata(IDeckLinkVideoInputFrame *frame, cf_dl_frame_info &info) {
    const double nan = std::nan("");
    info.colorspace = CF_DL_UNKNOWN;
    info.eotf = CF_DL_UNKNOWN;
    double *hdr[] = {&info.red_x, &info.red_y, &info.green_x, &info.green_y,
                     &info.blue_x, &info.blue_y, &info.white_x, &info.white_y,
                     &info.max_mastering, &info.min_mastering, &info.max_cll,
                     &info.max_fall};
    for (double *v : hdr) *v = nan;
    info.dolby_vision_bytes = CF_DL_UNKNOWN;

    IDeckLinkVideoFrameMetadataExtensions *meta = nullptr;
    if (frame->QueryInterface(IID_IDeckLinkVideoFrameMetadataExtensions, (void **)&meta) != S_OK || !meta)
        return;

    int64_t i = 0;
    if (meta->GetInt(bmdDeckLinkFrameMetadataColorspace, &i) == S_OK) info.colorspace = i;
    if (meta->GetInt(bmdDeckLinkFrameMetadataHDRElectroOpticalTransferFunc, &i) == S_OK) info.eotf = i;
    // With no buffer, GetBytes reports the size it would write.
    uint32_t dv = 0;
    if (meta->GetBytes(bmdDeckLinkFrameMetadataDolbyVision, nullptr, &dv) == S_OK && dv > 0)
        info.dolby_vision_bytes = dv;

    // The HDR values are only meaningful on a frame flagged as carrying them.
    if (frame->GetFlags() & bmdFrameContainsHDRMetadata) {
        const BMDDeckLinkFrameMetadataID ids[] = {
            bmdDeckLinkFrameMetadataHDRDisplayPrimariesRedX,
            bmdDeckLinkFrameMetadataHDRDisplayPrimariesRedY,
            bmdDeckLinkFrameMetadataHDRDisplayPrimariesGreenX,
            bmdDeckLinkFrameMetadataHDRDisplayPrimariesGreenY,
            bmdDeckLinkFrameMetadataHDRDisplayPrimariesBlueX,
            bmdDeckLinkFrameMetadataHDRDisplayPrimariesBlueY,
            bmdDeckLinkFrameMetadataHDRWhitePointX,
            bmdDeckLinkFrameMetadataHDRWhitePointY,
            bmdDeckLinkFrameMetadataHDRMaxDisplayMasteringLuminance,
            bmdDeckLinkFrameMetadataHDRMinDisplayMasteringLuminance,
            bmdDeckLinkFrameMetadataHDRMaximumContentLightLevel,
            bmdDeckLinkFrameMetadataHDRMaximumFrameAverageLightLevel,
        };
        static_assert(sizeof ids / sizeof ids[0] == sizeof hdr / sizeof hdr[0], "one ID per field");
        for (size_t k = 0; k < sizeof ids / sizeof ids[0]; k++) {
            double d = 0;
            if (meta->GetFloat(ids[k], &d) == S_OK) *hdr[k] = d;
        }
    }
    meta->Release();
}

// The SDK's callback object. COM-refcounted: the SDK holds a reference while it
// is registered, and cf_dl_input holds one for its lifetime.
//
// Every callback runs under mu_ and checks active_, and disarm() clears active_
// under the same lock. So once disarm() returns, no callback is inside Rust and
// none will enter it, even if the SDK delivers one late while streams stop. The
// C++ version learned this from a SIGSEGV in VideoInputFrameArrived during a
// mode change.
class Callback final : public IDeckLinkInputCallback {
public:
    explicit Callback(IDeckLinkInput *input) : input_(input) {}

    HRESULT QueryInterface(REFIID iid, void **out) override {
        if (same_iid(iid, IID_IDeckLinkInputCallback) || same_iid(iid, IID_IUnknown)) {
            AddRef();
            *out = this;
            return S_OK;
        }
        *out = nullptr;
        return E_NOINTERFACE;
    }
    ULONG AddRef() override { return ++refs_; }
    ULONG Release() override {
        const ULONG n = --refs_;
        if (n == 0) delete this;
        return n;
    }

    void arm(cf_dl_format_cb on_format, cf_dl_frame_cb on_frame, void *ctx,
             BMDDisplayMode mode, BMDPixelFormat pixel_format, BMDTimeScale scale) {
        std::lock_guard<std::mutex> g(mu_);
        on_format_ = on_format;
        on_frame_ = on_frame;
        ctx_ = ctx;
        mode_ = mode;
        pixel_format_ = pixel_format;
        scale_ = scale;
        active_ = true;
    }

    void disarm() {
        std::lock_guard<std::mutex> g(mu_);
        active_ = false;
    }

    uint64_t failed_restarts() const { return failed_restarts_.load(); }

    HRESULT VideoInputFormatChanged(BMDVideoInputFormatChangedEvents,
                                    IDeckLinkDisplayMode *mode,
                                    BMDDetectedVideoInputFormatFlags flags) override {
        if (!mode) return S_OK;
        std::lock_guard<std::mutex> g(mu_);
        if (!active_) return S_OK;

        BMDTimeValue duration = 0;
        BMDTimeScale scale = 0;
        if (mode->GetFrameRate(&duration, &scale) != S_OK) duration = scale = 0;

        cf_dl_format f;
        f.frame_duration = duration;
        f.time_scale = scale;
        f.width = static_cast<int32_t>(mode->GetWidth());
        f.height = static_cast<int32_t>(mode->GetHeight());
        f.display_mode = mode->GetDisplayMode();
        f.detected_flags = flags;
        f.enabled_mode = mode_;
        f.enabled_pixel_format = pixel_format_;

        // Stream time must be read in the mode's own scale. In nanoseconds a
        // 23.976 frame is 41,708,333.33 ticks, so deltas jitter by one and an
        // exact comparison reports phantom drops.
        scale_ = scale;

        const uint32_t want = on_format_(ctx_, &f);
        if (want == 0) return S_OK;

        // The documented pattern: restart on the new mode from inside this
        // callback. If enabling fails, the old mode and format stay recorded,
        // so the next change is compared against what is really running.
        input_->StopStreams();
        if (input_->EnableVideoInput(f.display_mode, want,
                                     bmdVideoInputEnableFormatDetection) == S_OK) {
            mode_ = f.display_mode;
            pixel_format_ = want;
        } else {
            failed_restarts_++;
        }
        input_->FlushStreams();
        input_->StartStreams();
        return S_OK;
    }

    HRESULT VideoInputFrameArrived(IDeckLinkVideoInputFrame *frame,
                                   IDeckLinkAudioInputPacket *) override {
        if (!frame) return S_OK;
        std::lock_guard<std::mutex> g(mu_);
        if (!active_) return S_OK;

        cf_dl_frame_info info;
        info.stream_time = CF_DL_UNKNOWN;
        info.stream_duration = CF_DL_UNKNOWN;
        BMDTimeValue t = 0, d = 0;
        if (scale_ > 0 && frame->GetStreamTime(&t, &d, scale_) == S_OK) {
            info.stream_time = t;
            info.stream_duration = d;
        }
        info.row_bytes = frame->GetRowBytes();
        info.width = static_cast<int32_t>(frame->GetWidth());
        info.height = static_cast<int32_t>(frame->GetHeight());
        info.pixel_format = frame->GetPixelFormat();
        info.flags = frame->GetFlags();
        read_metadata(frame, info);

        // The SDK only lends the frame for this call. The reference taken here
        // goes to Rust, which releases it when it drops the frame.
        frame->AddRef();
        on_frame_(ctx_, reinterpret_cast<cf_dl_frame *>(frame), &info);
        return S_OK;
    }

private:
    ~Callback() = default;

    IDeckLinkInput *input_;   // not owned: cf_dl_input outlives this object's use
    std::atomic<ULONG> refs_{1};
    std::atomic<uint64_t> failed_restarts_{0};

    std::mutex mu_;
    bool active_ = false;
    cf_dl_format_cb on_format_ = nullptr;
    cf_dl_frame_cb on_frame_ = nullptr;
    void *ctx_ = nullptr;
    BMDDisplayMode mode_ = kFallbackMode;
    BMDPixelFormat pixel_format_ = kFallbackPixelFormat;
    BMDTimeScale scale_ = 0;
};

} // namespace

struct cf_dl_input {
    IDeckLinkInput *input;
    IDeckLinkStatus *status;    /* null if the device has none */
    Callback *cb;
    bool started;               /* the owner's thread only; the exit handler goes by the set */
    int64_t opened_connection;  /* the input selected before open, or 0 */
};

namespace {

// Inputs started whose stop has not begun. A process that ends without
// stopping its input (std::process::exit on a second Ctrl-C, say) leaves the
// driver with it enabled; this stops them at exit instead. Heap objects never
// destroyed: statics are destroyed interleaved with atexit handlers in reverse
// order of completion, and these complete after the registration, so as
// ordinary statics they would be gone before the handler runs.
std::mutex &started_mu() {
    static std::mutex *m = new std::mutex;
    return *m;
}
std::set<cf_dl_input *> &started_inputs() {
    static std::set<cf_dl_input *> *s = new std::set<cf_dl_input *>;
    return *s;
}

// The stop itself, done by whoever took the input out of the started set, so
// it runs once even when the exit handler and cf_dl_input_stop race.
void stop_untracked(cf_dl_input *in) {
    // Disarm first: it waits out any callback in progress, including a
    // format-change restart, and turns away any that arrive while streams stop.
    in->cb->disarm();
    in->input->StopStreams();
    in->input->DisableVideoInput();
    in->input->SetCallback(nullptr);
}

// Holds the lock while it stops, so a cf_dl_input_release on another thread
// waits for the stop before it frees the input. An input whose stop is already
// underway (the hang a second Ctrl-C is for) is out of the set and left alone.
void stop_all_at_exit() {
    std::lock_guard<std::mutex> g(started_mu());
    for (cf_dl_input *in : started_inputs()) stop_untracked(in);
    started_inputs().clear();
}

void track_started(cf_dl_input *in) {
    static std::once_flag once;
    std::call_once(once, [] { std::atexit(stop_all_at_exit); });
    std::lock_guard<std::mutex> g(started_mu());
    started_inputs().insert(in);
}

// Whether this call took the input out of the set, and so must stop it.
bool untrack(cf_dl_input *in) {
    std::lock_guard<std::mutex> g(started_mu());
    return started_inputs().erase(in) == 1;
}

// The mode and pixel format to open in: the detected mode and the format the
// handler asks for, which tells it the format as a change would; or, with no
// signal detected or no status to read it from, the fallback.
void opening_mode(cf_dl_input *in, cf_dl_format_cb on_format, void *ctx,
                  BMDDisplayMode *mode, BMDPixelFormat *pixel_format) {
    *mode = kFallbackMode;
    *pixel_format = kFallbackPixelFormat;
    int64_t detected = 0, flags = 0;
    if (!in->status ||
        in->status->GetInt(bmdDeckLinkStatusDetectedVideoInputMode, &detected) != S_OK ||
        detected == 0 || detected == bmdModeUnknown ||
        in->status->GetInt(bmdDeckLinkStatusDetectedVideoInputFormatFlags, &flags) != S_OK)
        return;
    IDeckLinkDisplayMode *dm = nullptr;
    if (in->input->GetDisplayMode(static_cast<BMDDisplayMode>(detected), &dm) != S_OK || !dm)
        return;
    cf_dl_format f;
    memset(&f, 0, sizeof f);
    BMDTimeValue duration = 0;
    BMDTimeScale scale = 0;
    if (dm->GetFrameRate(&duration, &scale) != S_OK) duration = scale = 0;
    f.frame_duration = duration;
    f.time_scale = scale;
    f.width = static_cast<int32_t>(dm->GetWidth());
    f.height = static_cast<int32_t>(dm->GetHeight());
    f.display_mode = static_cast<BMDDisplayMode>(detected);
    f.detected_flags = static_cast<BMDDetectedVideoInputFormatFlags>(flags);
    // Nothing enabled yet, so the handler always names a format.
    f.enabled_mode = 0;
    f.enabled_pixel_format = 0;
    dm->Release();
    const uint32_t want = on_format(ctx, &f);
    if (want != 0) {
        *mode = static_cast<BMDDisplayMode>(detected);
        *pixel_format = want;
    }
}

} // namespace

extern "C" cf_dl_input *cf_dl_input_open(cf_dl_device *dev) noexcept {
    IDeckLink *device = reinterpret_cast<IDeckLink *>(dev);
    IDeckLinkInput *input = nullptr;
    if (device->QueryInterface(IID_IDeckLinkInput, (void **)&input) != S_OK || !input)
        return nullptr;
    // Select the HDMI input for this session, as the C++ version does, and
    // only if something else is selected: every change of the input is a
    // hotplug on the HDMI port, and the Denon upstream does not reliably
    // recover from one (it keeps sending the signal and its metadata with a
    // black picture until its input is cycled). Not written to the card's
    // preferences: whether to persist it is a separate decision.
    IDeckLinkConfiguration *cfg = nullptr;
    int64_t connection = 0;
    if (device->QueryInterface(IID_IDeckLinkConfiguration, (void **)&cfg) == S_OK && cfg) {
        if (cfg->GetInt(bmdDeckLinkConfigVideoInputConnection, &connection) != S_OK)
            connection = 0;
        else if (connection != bmdVideoConnectionHDMI)
            cfg->SetInt(bmdDeckLinkConfigVideoInputConnection, bmdVideoConnectionHDMI);
        cfg->Release();
    }
    IDeckLinkStatus *status = nullptr;
    if (device->QueryInterface(IID_IDeckLinkStatus, (void **)&status) != S_OK) status = nullptr;
    return new cf_dl_input{input, status, new Callback(input), false, connection};
}

extern "C" int32_t cf_dl_input_start(cf_dl_input *in, cf_dl_format_cb on_format,
                                     cf_dl_frame_cb on_frame, void *ctx,
                                     const char **failed_call) noexcept {
    if (in->started) return S_OK;

    BMDDisplayMode mode;
    BMDPixelFormat pixel_format;
    opening_mode(in, on_format, ctx, &mode, &pixel_format);

    // Frames arrive in the opening mode until any format change, so stream
    // times need its scale from the start.
    BMDTimeScale scale = 0;
    IDeckLinkDisplayMode *dm = nullptr;
    if (in->input->GetDisplayMode(mode, &dm) == S_OK && dm) {
        BMDTimeValue duration = 0;
        if (dm->GetFrameRate(&duration, &scale) != S_OK) scale = 0;
        dm->Release();
    }
    in->cb->arm(on_format, on_frame, ctx, mode, pixel_format, scale);

    HRESULT hr = in->input->SetCallback(in->cb);
    if (hr != S_OK) {
        *failed_call = "SetCallback";
        in->cb->disarm();
        return hr;
    }
    hr = in->input->EnableVideoInput(mode, pixel_format, bmdVideoInputEnableFormatDetection);
    if (hr != S_OK) {
        *failed_call = "EnableVideoInput";
        in->cb->disarm();
        in->input->SetCallback(nullptr);
        return hr;
    }
    hr = in->input->StartStreams();
    if (hr != S_OK) {
        *failed_call = "StartStreams";
        in->cb->disarm();
        in->input->DisableVideoInput();
        in->input->SetCallback(nullptr);
        return hr;
    }
    in->started = true;
    track_started(in);
    return S_OK;
}

extern "C" void cf_dl_input_stop(cf_dl_input *in) noexcept {
    if (!in->started) return;
    in->started = false;
    if (untrack(in)) stop_untracked(in);
}

extern "C" int64_t cf_dl_input_opened_connection(cf_dl_input *in) noexcept {
    return in->opened_connection;
}

extern "C" uint64_t cf_dl_input_failed_restarts(cf_dl_input *in) noexcept {
    return in->cb->failed_restarts();
}

extern "C" void cf_dl_input_release(cf_dl_input *in) noexcept {
    cf_dl_input_stop(in);
    in->cb->Release();
    if (in->status) in->status->Release();
    in->input->Release();
    delete in;
}

extern "C" void cf_dl_frame_release(cf_dl_frame *frame) noexcept {
    reinterpret_cast<IDeckLinkVideoInputFrame *>(frame)->Release();
}

extern "C" cf_dl_buffer *cf_dl_frame_read(cf_dl_frame *frame, const uint8_t **data,
                                          uint64_t *size) noexcept {
    IDeckLinkVideoBuffer *buf = nullptr;
    if (reinterpret_cast<IDeckLinkVideoInputFrame *>(frame)->QueryInterface(
            IID_IDeckLinkVideoBuffer, (void **)&buf) != S_OK || !buf)
        return nullptr;
    if (buf->StartAccess(bmdBufferAccessRead) != S_OK) {
        buf->Release();
        return nullptr;
    }
    void *p = nullptr;
    uint64_t n = 0;
    if (buf->GetBytes(&p) != S_OK || !p || buf->GetSize(&n) != S_OK) {
        buf->EndAccess(bmdBufferAccessRead);
        buf->Release();
        return nullptr;
    }
    *data = static_cast<const uint8_t *>(p);
    *size = n;
    return reinterpret_cast<cf_dl_buffer *>(buf);
}

extern "C" void cf_dl_buffer_end(cf_dl_buffer *b) noexcept {
    IDeckLinkVideoBuffer *buf = reinterpret_cast<IDeckLinkVideoBuffer *>(b);
    buf->EndAccess(bmdBufferAccessRead);
    buf->Release();
}
