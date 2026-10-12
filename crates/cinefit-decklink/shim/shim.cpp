#include "shim.h"

#include <cstddef>
#include <cstdio>
#include <cstdlib>

#include <dlfcn.h>

#include "DeckLinkAPI.h"
#include "DeckLinkAPIVersion.h"

// The Rust side declares this struct by hand and asserts the same numbers. Every
// field is checked, so a reorder fails the build as well as a resize.
static_assert(sizeof(cf_dl_device_info) == 288, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_device_info, display_name) == 0, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_device_info, model_name) == 128, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_device_info, persistent_id) == 256, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_device_info, video_io_support) == 264, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_device_info, input_connections) == 272, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_device_info, input_format_detection) == 280, "layout must match src/ffi.rs");
static_assert(offsetof(cf_dl_device_info, hdr_metadata) == 284, "layout must match src/ffi.rs");
// The oldest driver these headers work with. src/ffi.rs repeats it, so an SDK
// upgrade fails the build until both sides agree.
static_assert(BLACKMAGIC_DECKLINK_API_VERSION == 0x10000000, "update REQUIRED_API_VERSION in src/ffi.rs");

namespace {

IDeckLinkIterator *unwrap(cf_dl_iterator *p) { return reinterpret_cast<IDeckLinkIterator *>(p); }
IDeckLink *unwrap(cf_dl_device *p) { return reinterpret_cast<IDeckLink *>(p); }

// On Linux the SDK returns strings as malloc'd `const char *` that the caller
// owns, so each one is copied out and freed here rather than crossing into Rust.
// It is freed on every path, whether or not there is room to copy it.
void take_string(const char *s, char *dst, size_t len) {
    if (len != 0) snprintf(dst, len, "%s", s ? s : "");
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

extern "C" int cf_dl_api_version(int64_t *version, char *err, size_t err_len) noexcept {
    if (err_len) err[0] = '\0';

    // Load the driver before the SDK does. When it is missing, the SDK prints
    // dlerror() to stderr; this hands the reason to the caller instead. When it
    // is present, the SDK's own dlopen finds it already loaded. The name is the
    // one the SDK's dispatch layer uses, which it defines only in its .cpp.
    void *lib = dlopen("libDeckLinkAPI.so", RTLD_NOW | RTLD_GLOBAL);
    if (!lib) {
        const char *why = dlerror();
        if (err_len) snprintf(err, err_len, "%s", why ? why : "unknown dlopen error");
        return -1;
    }

    IDeckLinkAPIInformation *info = CreateDeckLinkAPIInformationInstance();
    dlclose(lib); // The SDK holds its own reference now.
    if (!info) {
        if (err_len) snprintf(err, err_len, "libDeckLinkAPI.so has no API information entry point");
        return -1;
    }
    if (info->GetInt(BMDDeckLinkAPIVersion, version) != S_OK)
        *version = CF_DL_UNKNOWN;
    info->Release();
    return 0;
}

extern "C" cf_dl_iterator *cf_dl_iterator_create(void) noexcept {
    return reinterpret_cast<cf_dl_iterator *>(CreateDeckLinkIteratorInstance());
}

extern "C" cf_dl_device *cf_dl_iterator_next(cf_dl_iterator *it) noexcept {
    IDeckLink *dev = nullptr;
    if (unwrap(it)->Next(&dev) != S_OK) return nullptr;
    return reinterpret_cast<cf_dl_device *>(dev);
}

extern "C" void cf_dl_iterator_release(cf_dl_iterator *it) noexcept {
    unwrap(it)->Release();
}

extern "C" void cf_dl_device_get_info(cf_dl_device *handle, cf_dl_device_info *out) noexcept {
    IDeckLink *dev = unwrap(handle);
    // No memset: the caller pre-fills *out as "unreported", so a field this
    // function does not set reads as unreported rather than as zero ("no").

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

extern "C" void cf_dl_device_release(cf_dl_device *dev) noexcept {
    unwrap(dev)->Release();
}
