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

/* Captured YCbCr 4:2:2 on the GPU: a luma plane (r16, w x h) and a chroma
 * plane (rg16, Cb and Cr interleaved, ceil(w/2) x h), each sample stored as
 * its 10-bit code shifted left by 6, plus the texture the packed frame is
 * uploaded into. Destroy with cf_pl_planes_destroy before cf_pl_destroy. */
typedef struct cf_pl_planes cf_pl_planes;

/* Null on failure, logged. */
cf_pl_planes *cf_pl_planes_create(cf_pl *pl, int w, int h);
void cf_pl_planes_destroy(cf_pl *pl, cf_pl_planes *planes);

/* Uploads a v210 frame of the planes' size and unpacks it into them on the
 * GPU. row_bytes is the capture card's stride, a multiple of 16 that covers
 * every 16-byte group of six pixels; data holds row_bytes * h bytes. Returns
 * 0, or -1 with the reason logged. */
int cf_pl_unpack_v210(cf_pl *pl, cf_pl_planes *planes, const uint8_t *data, size_t row_bytes);

/* Reads the planes back as stored, rows packed: w * h luma samples and
 * ceil(w/2) * h Cb, Cr pairs. For checks. Returns 0, or -1, logged. */
int cf_pl_planes_download(cf_pl *pl, cf_pl_planes *planes, uint16_t *luma, uint16_t *chroma);

/* Colour descriptions, as plain ints so nothing but this header is shared. */
#define CF_PL_MATRIX_BT709 0
#define CF_PL_MATRIX_BT2020_NC 1
#define CF_PL_PRIM_BT709 0
#define CF_PL_PRIM_BT2020 1
#define CF_PL_TRC_BT1886 0
#define CF_PL_TRC_PQ 1
#define CF_PL_TRC_HLG 2

/* Renders the planes over the whole of dst: limited-range YCbCr with the
 * given matrix, primaries and transfer, chroma co-sited as HDMI 4:2:2 is,
 * into full-range RGB with dst_primaries and dst_trc. No dithering; waits for
 * the GPU. Returns 0, or -1 with the reason logged. */
int cf_pl_render_ycbcr(cf_pl *pl, cf_pl_planes *src, int matrix, int primaries, int trc,
                       cf_pl_tex *dst, int dst_primaries, int dst_trc);

/* Captured RGB on the GPU: one rgba16 texture, w x h, each component stored
 * as its code shifted to the top of 16 bits (10-bit << 6, 8-bit << 8), plus
 * the texture the packed frame is uploaded into. Destroy with
 * cf_pl_rgb_destroy before cf_pl_destroy. */
typedef struct cf_pl_rgb cf_pl_rgb;

/* Null on failure, logged. */
cf_pl_rgb *cf_pl_rgb_create(cf_pl *pl, int w, int h);
void cf_pl_rgb_destroy(cf_pl *pl, cf_pl_rgb *rgb);

/* Capture formats cf_pl_unpack_rgb reads. */
#define CF_PL_R210 0  /* big-endian words: 2 unused bits, 10-bit R, G, B */
#define CF_PL_ARGB8 1 /* bytes A, R, G, B */

/* Uploads a frame of the texture's size in `format` and unpacks it on the
 * GPU. row_bytes is the capture card's stride, a multiple of 4 covering the
 * width; data holds row_bytes * h bytes. Returns 0, or -1, logged. */
int cf_pl_unpack_rgb(cf_pl *pl, cf_pl_rgb *rgb, const uint8_t *data, size_t row_bytes, int format);

/* Reads the texture back as stored: w * h * 4 values (R, G, B, and an opaque
 * alpha), rows packed. For checks. Returns 0, or -1, logged. */
int cf_pl_rgb_download(cf_pl *pl, cf_pl_rgb *rgb, uint16_t *out);

#define CF_PL_LEVELS_FULL 0
#define CF_PL_LEVELS_LIMITED 1

/* Renders the RGB over the whole of dst: the given range, primaries and
 * transfer, into full-range RGB with dst_primaries and dst_trc. Uses the bit
 * depth of the last unpack. No dithering; waits for the GPU. Returns 0, or
 * -1, logged. */
int cf_pl_render_rgb(cf_pl *pl, cf_pl_rgb *src, int levels, int primaries, int trc,
                     cf_pl_tex *dst, int dst_primaries, int dst_trc);

/* A w x h RGBA 16-bit texture to render into and read back, for checks. Null
 * on failure, logged. */
cf_pl_tex *cf_pl_target_rgba16(cf_pl *pl, int w, int h);

/* Reads a cf_pl_target_rgba16 texture back: w * h * 4 values, rows packed.
 * Returns 0, or -1, logged. */
int cf_pl_download_rgba16(cf_pl *pl, cf_pl_tex *tex, uint16_t *out);

#endif
