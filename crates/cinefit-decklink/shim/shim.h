/* C interface to the DeckLink SDK.
 *
 * The SDK is C++ with COM-style interfaces, which Rust cannot call directly.
 * This shim is the only code that includes DeckLinkAPI.h; everything it exposes
 * is plain C so the Rust side can declare it by hand. Keep it in step with
 * src/ffi.rs, which asserts the same struct layout. */
#ifndef CINEFIT_DECKLINK_SHIM_H
#define CINEFIT_DECKLINK_SHIM_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Rust calls these through `extern "C"`, which must not unwind. noexcept makes
 * an escaping C++ exception terminate here instead. Every declaration needs the
 * same exception specification, so the header carries it too. */
#ifdef __cplusplus
#define CF_DL_NOEXCEPT noexcept
#else
#define CF_DL_NOEXCEPT
#endif

/* Opaque handles. Each holds one SDK reference, dropped by its _release. */
typedef struct cf_dl_iterator cf_dl_iterator;
typedef struct cf_dl_device   cf_dl_device;

/* Attributes are tri-state: a device may not report one at all, and "not
 * reported" must not read as "no". */
#define CF_DL_UNKNOWN (-1)

typedef struct {
    char    display_name[128];
    char    model_name[128];
    int64_t persistent_id;           /* CF_DL_UNKNOWN if unreported */
    int64_t video_io_support;        /* BMDVideoIOSupport bits, or CF_DL_UNKNOWN */
    int64_t input_connections;       /* BMDVideoConnection bits, or CF_DL_UNKNOWN */
    int32_t input_format_detection;  /* 1, 0, or CF_DL_UNKNOWN */
    int32_t hdr_metadata;            /* 1, 0, or CF_DL_UNKNOWN */
} cf_dl_device_info;

/* Writes the driver's API version into *version, encoded like the SDK's
 * BLACKMAGIC_DECKLINK_API_VERSION (0xMMmmpp00), or CF_DL_UNKNOWN if the driver
 * does not report it. Returns 0, or -1 if the driver library could not be
 * loaded, with the reason written into err. Call this before anything else:
 * it is what keeps the SDK from printing load failures to stderr. */
int cf_dl_api_version(int64_t *version, char *err, size_t err_len) CF_DL_NOEXCEPT;

/* Null if the driver library could not be loaded, or is older than the
 * headers the shim was built with. */
cf_dl_iterator *cf_dl_iterator_create(void) CF_DL_NOEXCEPT;
/* The next device, or null when there are no more. */
cf_dl_device *cf_dl_iterator_next(cf_dl_iterator *it) CF_DL_NOEXCEPT;
void cf_dl_iterator_release(cf_dl_iterator *it) CF_DL_NOEXCEPT;

/* Fills the fields of *out that the device reports. The caller initialises
 * *out first (every field CF_DL_UNKNOWN, names empty): fields left untouched
 * keep that value. */
void cf_dl_device_get_info(cf_dl_device *dev, cf_dl_device_info *out) CF_DL_NOEXCEPT;
void cf_dl_device_release(cf_dl_device *dev) CF_DL_NOEXCEPT;

/* Reads the BMDDynamicRange bits the input EDID advertises into *bits.
 * Returns 0, or -1 if the device has no EDID interface or does not say.
 * Not part of cf_dl_device_get_info on purpose: taking and releasing the EDID
 * interface may itself be a hotplug upstream (the card restores its default
 * EDID when the interface is released), and the Denon in this chain does not
 * reliably recover from one. Call it only when asked to. */
int32_t cf_dl_device_read_input_edid(cf_dl_device *dev, int64_t *bits) CF_DL_NOEXCEPT;

/* ---- Capture (input.cpp) ---- */

typedef struct cf_dl_input cf_dl_input;
/* One captured frame. Holds one SDK reference, dropped by cf_dl_frame_release;
 * the card's buffer is not reused until it is. */
typedef struct cf_dl_frame cf_dl_frame;

/* The input's format, as detected when the signal changes. */
typedef struct {
    int64_t  frame_duration;        /* rate = time_scale / frame_duration fps, */
    int64_t  time_scale;            /* e.g. 24000 / 1001; 0 if unreported */
    int32_t  width;
    int32_t  height;
    uint32_t display_mode;          /* BMDDisplayMode detected */
    uint32_t detected_flags;        /* BMDDetectedVideoInputFormatFlags */
    uint32_t enabled_mode;          /* BMDDisplayMode the input runs in now */
    uint32_t enabled_pixel_format;  /* BMDPixelFormat the input runs in now */
} cf_dl_format;

typedef struct {
    int64_t  stream_time;           /* in the format's time_scale, or CF_DL_UNKNOWN */
    int64_t  stream_duration;       /* likewise */
    int64_t  row_bytes;             /* the SDK's stride; never compute it */
    int32_t  width;
    int32_t  height;
    uint32_t pixel_format;          /* BMDPixelFormat */
    uint32_t flags;                 /* BMDFrameFlags */
    /* Per-frame metadata. Integers are CF_DL_UNKNOWN and floats NaN when the
     * frame does not report them; the HDR values are read only when the frame
     * is flagged bmdFrameContainsHDRMetadata, so none are stale. */
    int64_t  colorspace;            /* BMDColorspace */
    int64_t  eotf;                  /* 0-7, as in CTA-861.3 */
    double   red_x, red_y;          /* mastering display primaries, 0-1 */
    double   green_x, green_y;
    double   blue_x, blue_y;
    double   white_x, white_y;
    double   max_mastering;         /* cd/m2 */
    double   min_mastering;         /* cd/m2 */
    double   max_cll;               /* cd/m2 */
    double   max_fall;              /* cd/m2 */
} cf_dl_frame_info;

/* Both callbacks run on the SDK's thread and must return quickly: stalling
 * them makes the card drop frames. */

/* Returns the BMDPixelFormat to re-enable the input with on the detected mode,
 * or 0 to leave it running as it is. Also called once from cf_dl_input_start,
 * on the caller's thread, with enabled_mode 0, to choose the opening format.
 * An input left running at process exit is stopped by an atexit handler. */
typedef uint32_t (*cf_dl_format_cb)(void *ctx, const cf_dl_format *format);
/* Hands over one reference to frame; the callee releases it. */
typedef void (*cf_dl_frame_cb)(void *ctx, cf_dl_frame *frame, const cf_dl_frame_info *info);

/* Null if the device has no capture input. */
cf_dl_input *cf_dl_input_open(cf_dl_device *dev) CF_DL_NOEXCEPT;

/* Enables the input with format detection and starts it: in the mode the card
 * already detects, with on_format called once first to choose the pixel
 * format, or in a fallback mode if it detects none. Returns 0, or the
 * failing call's HRESULT with its name in *failed_call. E_ACCESSDENIED from
 * EnableVideoInput means another program holds the input. Callbacks may run
 * from here until cf_dl_input_stop returns, with ctx passed through. */
int32_t cf_dl_input_start(cf_dl_input *in, cf_dl_format_cb on_format,
                          cf_dl_frame_cb on_frame, void *ctx,
                          const char **failed_call) CF_DL_NOEXCEPT;

/* Stops the input. When it returns, no callback is running or will run. */
void cf_dl_input_stop(cf_dl_input *in) CF_DL_NOEXCEPT;

/* How many format-change restarts failed to enable the new mode. */
uint64_t cf_dl_input_failed_restarts(cf_dl_input *in) CF_DL_NOEXCEPT;
/* The BMDVideoConnection selected when the input was opened, before HDMI was
 * selected for the session: the card's saved preference. 0 if unreadable. */
int64_t cf_dl_input_opened_connection(cf_dl_input *in) CF_DL_NOEXCEPT;

/* Stops the input if it is running, then frees it. */
void cf_dl_input_release(cf_dl_input *in) CF_DL_NOEXCEPT;

void cf_dl_frame_release(cf_dl_frame *frame) CF_DL_NOEXCEPT;

/* Read access to a frame's pixels. SDK 16 only allows it between StartAccess
 * and EndAccess, so access is a handle: *data and *size stay valid until
 * cf_dl_buffer_end, which the caller must reach before releasing the frame.
 * Null if the frame's buffer cannot be read. */
typedef struct cf_dl_buffer cf_dl_buffer;
cf_dl_buffer *cf_dl_frame_read(cf_dl_frame *frame, const uint8_t **data,
                               uint64_t *size) CF_DL_NOEXCEPT;
void cf_dl_buffer_end(cf_dl_buffer *buf) CF_DL_NOEXCEPT;

#ifdef __cplusplus
}
#endif

#endif
