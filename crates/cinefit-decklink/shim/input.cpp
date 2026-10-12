// Capture: the input, its SDK callback, and the restart on a format change.
// Which pixel format to capture is decided in Rust; this file only carries out
// the decision.
#include "shim.h"

#include <atomic>
#include <cmath>
#include <cstddef>
#include <cstring>
#include <mutex>

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

static_assert(sizeof(cf_dl_frame_info) == 152, "layout must match src/ffi.rs");
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

namespace {

// The input starts in a mode no source here sends, so format detection always
// reports the real format at least once. Starting in the mode that arrives
// means no change is ever reported, and the detected sampling and bit depth
// are never seen: the first capture run, of a 2160p23.98 source, showed that.
constexpr BMDDisplayMode kInitialMode = bmdModeHD720p50;
constexpr BMDPixelFormat kInitialPixelFormat = bmdFormat10BitYUV;

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

    IDeckLinkVideoFrameMetadataExtensions *meta = nullptr;
    if (frame->QueryInterface(IID_IDeckLinkVideoFrameMetadataExtensions, (void **)&meta) != S_OK || !meta)
        return;

    int64_t i = 0;
    if (meta->GetInt(bmdDeckLinkFrameMetadataColorspace, &i) == S_OK) info.colorspace = i;
    if (meta->GetInt(bmdDeckLinkFrameMetadataHDRElectroOpticalTransferFunc, &i) == S_OK) info.eotf = i;

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
             BMDTimeScale scale) {
        std::lock_guard<std::mutex> g(mu_);
        on_format_ = on_format;
        on_frame_ = on_frame;
        ctx_ = ctx;
        mode_ = kInitialMode;
        pixel_format_ = kInitialPixelFormat;
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
    BMDDisplayMode mode_ = kInitialMode;
    BMDPixelFormat pixel_format_ = kInitialPixelFormat;
    BMDTimeScale scale_ = 0;
};

} // namespace

struct cf_dl_input {
    IDeckLinkInput *input;
    Callback *cb;
    bool started;
};

extern "C" cf_dl_input *cf_dl_input_open(cf_dl_device *dev) noexcept {
    IDeckLinkInput *input = nullptr;
    if (reinterpret_cast<IDeckLink *>(dev)->QueryInterface(IID_IDeckLinkInput, (void **)&input) != S_OK || !input)
        return nullptr;
    return new cf_dl_input{input, new Callback(input), false};
}

extern "C" int32_t cf_dl_input_start(cf_dl_input *in, cf_dl_format_cb on_format,
                                     cf_dl_frame_cb on_frame, void *ctx,
                                     const char **failed_call) noexcept {
    if (in->started) return S_OK;

    // Frames can arrive before the first format change, in the initial mode,
    // so stream times need its scale from the start.
    BMDTimeScale scale = 0;
    IDeckLinkDisplayMode *dm = nullptr;
    if (in->input->GetDisplayMode(kInitialMode, &dm) == S_OK && dm) {
        BMDTimeValue duration = 0;
        if (dm->GetFrameRate(&duration, &scale) != S_OK) scale = 0;
        dm->Release();
    }
    in->cb->arm(on_format, on_frame, ctx, scale);

    HRESULT hr = in->input->SetCallback(in->cb);
    if (hr != S_OK) {
        *failed_call = "SetCallback";
        in->cb->disarm();
        return hr;
    }
    hr = in->input->EnableVideoInput(kInitialMode, kInitialPixelFormat,
                                     bmdVideoInputEnableFormatDetection);
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
    return S_OK;
}

extern "C" void cf_dl_input_stop(cf_dl_input *in) noexcept {
    if (!in->started) return;
    // Disarm first: it waits out any callback in progress, including a
    // format-change restart, and turns away any that arrive while streams stop.
    in->cb->disarm();
    in->input->StopStreams();
    in->input->DisableVideoInput();
    in->input->SetCallback(nullptr);
    in->started = false;
}

extern "C" uint64_t cf_dl_input_failed_restarts(cf_dl_input *in) noexcept {
    return in->cb->failed_restarts();
}

extern "C" void cf_dl_input_release(cf_dl_input *in) noexcept {
    cf_dl_input_stop(in);
    in->cb->Release();
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
