#include "shim.h"

#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <cstring>

#include "DeckLinkAPI.h"

// The Rust side declares this struct by hand and asserts the same numbers.
static_assert(sizeof(cf_dl_device_info) == 288, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_device_info, persistent_id) == 256, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_device_info, input_format_detection) == 280, "layout must match src/ffi.rs");

namespace {

IDeckLinkIterator *unwrap(cf_dl_iterator *p) { return reinterpret_cast<IDeckLinkIterator *>(p); }
IDeckLink *unwrap(cf_dl_device *p) { return reinterpret_cast<IDeckLink *>(p); }

// On Linux the SDK returns strings as malloc'd `const char *` that the caller
// owns, so each one is copied out and freed here rather than crossing into Rust.
void take_string(const char *s, char *dst, size_t len) {
    if (len == 0) return;
    dst[0] = '\0';
    if (!s) return;
    snprintf(dst, len, "%s", s);
    free(const_cast<char *>(s));
}

int32_t flag(IDeckLinkProfileAttributes *a, BMDDeckLinkAttributeID id) {
    bool v = false;
    if (!a || a->GetFlag(id, &v) != S_OK) return CF_DL_UNKNOWN;
    return v ? 1 : 0;
}

int64_t integer(IDeckLinkProfileAttributes *a, BMDDeckLinkAttributeID id) {
    int64_t v = 0;
    if (!a || a->GetInt(id, &v) != S_OK) return CF_DL_UNKNOWN;
    return v;
}

} // namespace

extern "C" int cf_dl_api_version(char *buf, size_t len) {
    // Null when libDeckLinkAPI.so is missing: the driver is not installed.
    IDeckLinkAPIInformation *info = CreateDeckLinkAPIInformationInstance();
    if (!info) return -1;
    const char *s = nullptr;
    if (info->GetString(BMDDeckLinkAPIVersion, &s) == S_OK)
        take_string(s, buf, len);
    else if (len)
        buf[0] = '\0';
    info->Release();
    return 0;
}

extern "C" cf_dl_iterator *cf_dl_iterator_create(void) {
    return reinterpret_cast<cf_dl_iterator *>(CreateDeckLinkIteratorInstance());
}

extern "C" cf_dl_device *cf_dl_iterator_next(cf_dl_iterator *it) {
    IDeckLink *dev = nullptr;
    if (unwrap(it)->Next(&dev) != S_OK) return nullptr;
    return reinterpret_cast<cf_dl_device *>(dev);
}

extern "C" void cf_dl_iterator_release(cf_dl_iterator *it) {
    unwrap(it)->Release();
}

extern "C" void cf_dl_device_get_info(cf_dl_device *handle, cf_dl_device_info *out) {
    IDeckLink *dev = unwrap(handle);
    memset(out, 0, sizeof *out);

    const char *s = nullptr;
    if (dev->GetDisplayName(&s) == S_OK)
        take_string(s, out->display_name, sizeof out->display_name);
    s = nullptr;
    if (dev->GetModelName(&s) == S_OK)
        take_string(s, out->model_name, sizeof out->model_name);

    IDeckLinkProfileAttributes *attrs = nullptr;
    if (dev->QueryInterface(IID_IDeckLinkProfileAttributes, (void **)&attrs) != S_OK)
        attrs = nullptr;
    out->persistent_id          = integer(attrs, BMDDeckLinkPersistentID);
    out->video_io_support       = integer(attrs, BMDDeckLinkVideoIOSupport);
    out->input_connections      = integer(attrs, BMDDeckLinkVideoInputConnections);
    out->input_format_detection = flag(attrs, BMDDeckLinkSupportsInputFormatDetection);
    out->hdr_metadata           = flag(attrs, BMDDeckLinkSupportsHDRMetadata);
    if (attrs) attrs->Release();
}

extern "C" void cf_dl_device_release(cf_dl_device *dev) {
    unwrap(dev)->Release();
}
