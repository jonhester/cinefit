/* C interface to libplacebo.
 *
 * libplacebo has no maintained Rust binding, so this shim is the only code
 * that includes its headers; everything it exposes is plain C, declared by
 * hand in src/ffi.rs. Keep the two in step. */
#ifndef CINEFIT_PLACEBO_SHIM_H
#define CINEFIT_PLACEBO_SHIM_H

#include <stddef.h>
#include <stdint.h>

typedef struct cf_pl cf_pl;

/* level is libplacebo's: 1 fatal, 2 error, 3 warning, 4 info, 5 debug. */
typedef void (*cf_pl_log_cb)(void *ctx, int level, const char *msg);

/* The libplacebo API version the shim was built against (PL_API_VER). */
int cf_pl_api_version(void);

/* Creates libplacebo's Vulkan device, logging warnings and worse through
 * log_cb. Null on failure, with the reason in err. */
cf_pl *cf_pl_create(cf_pl_log_cb log_cb, void *ctx, char *err, size_t err_len);
void cf_pl_destroy(cf_pl *pl);

typedef struct {
    char     device_name[256];
    uint32_t vulkan_api_version;     /* VK_MAKE_API_VERSION encoding */
    uint32_t max_tex_2d_dim;
    int32_t  dma_buf_import;         /* 1 if textures import from dma-bufs */
} cf_pl_info;

void cf_pl_get_info(cf_pl *pl, cf_pl_info *out);

/* Renders a w x h RGBA8 image (rows packed, 4 bytes a pixel) through
 * pl_render_image into an RGBA8 target of the same size, with no dithering,
 * and reads the result back into out. Returns 0, or -1 with the reason
 * logged. */
int cf_pl_render_rgba8(cf_pl *pl, int w, int h, const uint8_t *in, uint8_t *out);

/* A libplacebo texture. Destroy with cf_pl_tex_destroy before cf_pl_destroy. */
typedef struct cf_pl_tex cf_pl_tex;

/* Imports a dma-buf as a render target: one plane of DRM format `fourcc`
 * (e.g. XRGB8888) with the given modifier, stride and offset. libplacebo
 * duplicates fd, so the caller keeps and closes its own. Null on failure,
 * logged. */
cf_pl_tex *cf_pl_import_dmabuf(cf_pl *pl, int fd, int w, int h, uint32_t fourcc,
                               uint64_t modifier, uint32_t stride, uint32_t offset);

/* Uploads a w x h RGBA8 image (rows packed) as a texture to render from.
 * Null on failure, logged. */
cf_pl_tex *cf_pl_upload_rgba8(cf_pl *pl, int w, int h, const uint8_t *rgba);

void cf_pl_tex_destroy(cf_pl *pl, cf_pl_tex *tex);

/* Renders src over the whole of dst, both sRGB, without dithering, and waits
 * for the GPU to finish. shift_x scrolls the source left by that many pixels,
 * wrapping around, for a pattern that moves. Returns 0, or -1 with the
 * reason logged. */
int cf_pl_render(cf_pl *pl, cf_pl_tex *src, cf_pl_tex *dst, float shift_x);

#endif
