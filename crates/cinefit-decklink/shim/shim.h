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

/* Writes the driver's API version into buf. Returns 0, or -1 if the driver
 * library could not be loaded. */
int cf_dl_api_version(char *buf, size_t len);

/* Null if the driver library could not be loaded. */
cf_dl_iterator *cf_dl_iterator_create(void);
/* The next device, or null when there are no more. */
cf_dl_device *cf_dl_iterator_next(cf_dl_iterator *it);
void cf_dl_iterator_release(cf_dl_iterator *it);

void cf_dl_device_get_info(cf_dl_device *dev, cf_dl_device_info *out);
void cf_dl_device_release(cf_dl_device *dev);

#ifdef __cplusplus
}
#endif

#endif
