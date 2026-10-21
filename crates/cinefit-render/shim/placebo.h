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
/* Fills a render target with black, for a frame with nothing to show. */
int cf_pl_clear_black(cf_pl *pl, cf_pl_tex *dst);
int cf_pl_render(cf_pl *pl, cf_pl_tex *src, cf_pl_tex *dst, float shift_x);

/* Captured YCbCr 4:2:2 on the GPU: a luma plane (r16, w x h) and a chroma
 * plane (rg16, Cb and Cr interleaved, ceil(w/2) x h), each sample stored as
 * its code shifted to the top of 16 bits (v210's 10 bits by 6, a tunnel's
 * 12 by 4), plus the texture the packed frame is uploaded into. Destroy with cf_pl_planes_destroy before cf_pl_destroy. */
typedef struct cf_pl_planes cf_pl_planes;

/* Null on failure, logged. */
cf_pl_planes *cf_pl_planes_create(cf_pl *pl, int w, int h);
void cf_pl_planes_destroy(cf_pl *pl, cf_pl_planes *planes);

/* Uploads a v210 frame of the planes' size and unpacks it into them on the
 * GPU. row_bytes is the capture card's stride, a multiple of 16 that covers
 * every 16-byte group of six pixels; data holds row_bytes * h bytes.
 * chroma_bias, if not NULL, is two floats added to every Cb and Cr code, in
 * 10-bit codes, as the planes store them (a capture chain's known bias). Returns
 * 0, or -1 with the reason logged. */
int cf_pl_unpack_v210(cf_pl *pl, cf_pl_planes *planes, const uint8_t *data, size_t row_bytes,
                      const float *chroma_bias);

/* Reads the planes back as stored, rows packed: w * h luma samples and
 * ceil(w/2) * h Cb, Cr pairs. For checks. Returns 0, or -1, logged; -1 also
 * before the first successful unpack, as cf_pl_render_ycbcr does. */
int cf_pl_planes_download(cf_pl *pl, cf_pl_planes *planes, uint16_t *luma, uint16_t *chroma);

/* Colour descriptions, as plain ints so nothing but this header is shared.
 *
 * Every render below takes `place`: x0, y0, x1, y1 in the target's pixels,
 * where the picture goes, the rest cleared to black; NULL for the whole
 * target. */
#define CF_PL_MATRIX_BT709 0
#define CF_PL_MATRIX_BT2020_NC 1
#define CF_PL_PRIM_BT709 0
#define CF_PL_PRIM_BT2020 1
#define CF_PL_TRC_BT1886 0
#define CF_PL_TRC_PQ 1
#define CF_PL_TRC_HLG 2

/* A mapping between brightness ranges, for a render into a target that cannot
 * show the source's: the source's peak and the target's, in nits (0 leaves
 * libplacebo's default for that side), and the tone curve between them, one
 * of CF_PL_CURVE_*. NULL wherever one is taken means no mapping beyond what
 * the colour spaces imply, as before. peak_detect nonzero measures each frame
 * (libplacebo's peak detection, its default parameters), so the curve follows
 * the scene instead of the title's static peak. contrast_recovery is
 * libplacebo's, 0 (off) to 1: detail the curve flattened, added back.
 * peak_percentile is where peak detection reads the scene's peak (0 or 100:
 * the brightest pixel, libplacebo's default), knee_offset BT.2390's knee
 * offset (0: libplacebo's default). step nonzero maps with step_v in place of
 * the curve: in PQ scaled by each side's peak, the output at input 0, 1/16 ..
 * 1, linear between (Dolby's step between two of its own targets). */
#define CF_PL_STEP_POINTS 17
typedef struct {
    float src_peak;
    float dst_peak;
    int curve;
    int peak_detect;
    float contrast_recovery;
    float peak_percentile;
    float knee_offset;
    int step;
    float step_v[CF_PL_STEP_POINTS];
} cf_pl_map;

#define CF_PL_CURVE_CLIP 0
#define CF_PL_CURVE_HABLE 1
#define CF_PL_CURVE_SPLINE 2
#define CF_PL_CURVE_BT2390 3
#define CF_PL_CURVE_BT2446A 4
#define CF_PL_CURVE_ST2094_10 5
#define CF_PL_CURVE_ST2094_40 6
#define CF_PL_CURVE_MOBIUS 7
#define CF_PL_CURVE_REINHARD 8

/* `src_crop`, on every render: which part of the source is drawn, as x0, y0,
 * x1, y1 in source pixels, or NULL for the whole frame. The bars cropped out.
 *
 * Renders the planes over the whole of dst: limited-range YCbCr with the
 * given matrix, primaries and transfer, chroma co-sited as HDMI 4:2:2 is,
 * into full-range RGB with dst_primaries and dst_trc. No dithering; waits for
 * the GPU. Returns 0, or -1 with the reason logged. */
int cf_pl_render_ycbcr(cf_pl *pl, cf_pl_planes *src, int matrix, int primaries, int trc,
                       cf_pl_tex *dst, int dst_primaries, int dst_trc, const float *place,
                       const cf_pl_map *map, const float *src_crop);

/* Uploads a tunneled Dolby Vision frame, captured as r210, into planes of
 * its size and unpacks the 12-bit ICtCp 4:2:2 inside it on the GPU: codes
 * stored shifted left by 4. The width must be even. Returns 0, or -1,
 * logged. */
int cf_pl_unpack_tunnel(cf_pl *pl, cf_pl_planes *planes, const uint8_t *data, size_t row_bytes);

/* A tunnel frame's conversion, from its metadata. Matrices row-major, rows
 * the outputs; the offset is subtracted from the normalised signal. */
typedef struct {
    float ycc_to_lms[9];
    float offset[3];
    float linear[9];
} cf_pl_dovi;

/* Renders planes holding a tunnel through libplacebo's Dolby Vision
 * conversion (identity reshaping) to BT.2020 PQ, then into dst's primaries
 * and transfer. No dithering; waits for the GPU. Returns 0, or -1, logged. */
int cf_pl_render_dovi(cf_pl *pl, cf_pl_planes *src, const cf_pl_dovi *dovi,
                      cf_pl_tex *dst, int dst_primaries, int dst_trc, const float *place,
                      const cf_pl_map *map, const float *src_crop);

/* Points in a cf_pl_dm intensity table. */
#define CF_PL_DM_LUT 1025

/* One frame's Dolby display mapping (src/dm.rs): the intensity table over PQ
 * 0-1 and the per-pixel constants, in ICtCp. Hue terms in radians. */
typedef struct {
    float lut[CF_PL_DM_LUT];
    float k1, chroma_weight, saturation_gain, compression, tmax;
    float cs[4];            /* pull on over-bright colour: gain, chroma, power, start */
    float hue_s1;
    float hue_fix[4];
    float hue_cmp[6];
} cf_pl_dm;

/* As cf_pl_render_dovi, with Dolby's display mapping in place of
 * libplacebo's: the tunnel to BT.2020 PQ at the source's size, the mapping
 * applied there, then into dst with map, which should be one to one at the
 * mapping's target (src_peak = dst_peak). Returns 0, or -1, logged. */
int cf_pl_render_dovi_dm(cf_pl *pl, cf_pl_planes *src, const cf_pl_dovi *dovi,
                         const cf_pl_dm *dm, cf_pl_tex *dst, int dst_primaries, int dst_trc,
                         const float *place, const cf_pl_map *map, const float *src_crop);

/* The mapping alone, on w x h BT.2020 PQ RGBA16 pixels (rows packed, alpha
 * ignored) into out, the same size. For checking the GPU against src/dm.rs.
 * Returns 0, or -1, logged. */
int cf_pl_dm_rgba16(cf_pl *pl, int w, int h, const uint16_t *in, const cf_pl_dm *dm,
                    uint16_t *out);

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
 * alpha), rows packed. For checks. Returns 0, or -1, logged; -1 also before
 * the first successful unpack, as cf_pl_render_rgb does. */
int cf_pl_rgb_download(cf_pl *pl, cf_pl_rgb *rgb, uint16_t *out);

#define CF_PL_LEVELS_FULL 0
#define CF_PL_LEVELS_LIMITED 1

/* Renders the RGB over the whole of dst: the given range, primaries and
 * transfer, into full-range RGB with dst_primaries and dst_trc. Uses the bit
 * depth of the last unpack. No dithering; waits for the GPU. Returns 0, or
 * -1, logged. */
int cf_pl_render_rgb(cf_pl *pl, cf_pl_rgb *src, int levels, int primaries, int trc,
                     cf_pl_tex *dst, int dst_primaries, int dst_trc, const float *place,
                     const float *src_crop);

/* A w x h RGBA 16-bit texture to render into and read back, for checks. Null
 * on failure, logged. */
cf_pl_tex *cf_pl_target_rgba16(cf_pl *pl, int w, int h);

/* Reads a cf_pl_target_rgba16 texture back: w * h * 4 values, rows packed.
 * Returns 0, or -1, logged. */
int cf_pl_download_rgba16(cf_pl *pl, cf_pl_tex *tex, uint16_t *out);

#endif
