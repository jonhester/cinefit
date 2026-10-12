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

void cf_dl_device_get_info(cf_dl_device *dev, cf_dl_device_info *out) CF_DL_NOEXCEPT;
void cf_dl_device_release(cf_dl_device *dev) CF_DL_NOEXCEPT;

#ifdef __cplusplus
}
#endif

#endif
