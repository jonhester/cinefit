#include "placebo.h"

#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <libplacebo/config.h>
#include <libplacebo/dispatch.h>
#include <libplacebo/log.h>
#include <libplacebo/renderer.h>
#include <libplacebo/shaders/custom.h>
#include <libplacebo/vulkan.h>

/* The API the shim is written against. A different libplacebo fails the build
 * here rather than misbehaving at run time: update the shim, then this. */
_Static_assert(PL_API_VER == 375, "shim written for libplacebo API 375 (scripts/build-libplacebo.sh)");

/* cf_pl_map as src/ffi.rs declares it. */
_Static_assert(sizeof(cf_pl_map) == 16, "cf_pl_map size");
_Static_assert(offsetof(cf_pl_map, curve) == 8, "cf_pl_map.curve");
_Static_assert(offsetof(cf_pl_map, peak_detect) == 12, "cf_pl_map.peak_detect");

struct cf_pl {
    pl_log log;
    pl_vulkan vk;
    pl_renderer renderer;
    pl_dispatch dispatch;
    cf_pl_log_cb log_cb;
    void *log_ctx;
};

static void forward_log(void *priv, enum pl_log_level level, const char *msg) {
    struct cf_pl *pl = priv;
    if (pl->log_cb) pl->log_cb(pl->log_ctx, (int)level, msg);
}

int cf_pl_api_version(void) { return PL_API_VER; }

cf_pl *cf_pl_create(cf_pl_log_cb log_cb, void *ctx, char *err, size_t err_len) {
    if (err_len) err[0] = '\0';
    struct cf_pl *pl = calloc(1, sizeof *pl);
    if (!pl) {
        if (err_len) snprintf(err, err_len, "out of memory");
        return NULL;
    }
    pl->log_cb = log_cb;
    pl->log_ctx = ctx;
    pl->log = pl_log_create(PL_API_VER, pl_log_params(
        .log_cb = forward_log,
        .log_priv = pl,
        .log_level = PL_LOG_WARN,
    ));

    struct pl_vulkan_params params = pl_vulkan_default_params;
    pl->vk = pl_vulkan_create(pl->log, &params);
    if (!pl->vk) {
        if (err_len) snprintf(err, err_len, "pl_vulkan_create failed (see the log)");
        cf_pl_destroy(pl);
        return NULL;
    }
    pl->renderer = pl_renderer_create(pl->log, pl->vk->gpu);
    if (!pl->renderer) {
        if (err_len) snprintf(err, err_len, "pl_renderer_create failed");
        cf_pl_destroy(pl);
        return NULL;
    }
    pl->dispatch = pl_dispatch_create(pl->log, pl->vk->gpu);
    if (!pl->dispatch) {
        if (err_len) snprintf(err, err_len, "pl_dispatch_create failed");
        cf_pl_destroy(pl);
        return NULL;
    }
    return pl;
}

void cf_pl_destroy(cf_pl *pl) {
    if (!pl) return;
    pl_dispatch_destroy(&pl->dispatch);
    pl_renderer_destroy(&pl->renderer);
    pl_vulkan_destroy(&pl->vk);
    pl_log_destroy(&pl->log);
    free(pl);
}

void cf_pl_get_info(cf_pl *pl, cf_pl_info *out) {
    memset(out, 0, sizeof *out);
    const struct pl_vulkan_t *vk = pl->vk;
    PFN_vkGetPhysicalDeviceProperties props_fn = (PFN_vkGetPhysicalDeviceProperties)
        vk->get_proc_addr(vk->instance, "vkGetPhysicalDeviceProperties");
    if (props_fn) {
        VkPhysicalDeviceProperties p;
        props_fn(vk->phys_device, &p);
        snprintf(out->device_name, sizeof out->device_name, "%s", p.deviceName);
    }
    out->vulkan_api_version = vk->api_version;
    out->max_tex_2d_dim = vk->gpu->limits.max_tex_2d_dim;
    out->dma_buf_import = (vk->gpu->import_caps.tex & PL_HANDLE_DMA_BUF) ? 1 : 0;
}

int cf_pl_render_rgba8(cf_pl *pl, int w, int h, const uint8_t *in, uint8_t *out) {
    pl_gpu gpu = pl->vk->gpu;
    pl_fmt fmt = pl_find_named_fmt(gpu, "rgba8");
    if (!fmt) {
        if (pl->log_cb) pl->log_cb(pl->log_ctx, PL_LOG_ERR, "no rgba8 texture format");
        return -1;
    }
    pl_tex src = pl_tex_create(gpu, pl_tex_params(
        .w = w, .h = h, .format = fmt, .sampleable = true, .host_writable = true));
    pl_tex dst = pl_tex_create(gpu, pl_tex_params(
        .w = w, .h = h, .format = fmt, .renderable = true, .host_readable = true));
    int rc = -1;
    if (!src || !dst) goto done;

    if (!pl_tex_upload(gpu, pl_tex_transfer_params(
            .tex = src, .ptr = (void *)in, .row_pitch = (size_t)w * 4)))
        goto done;

    // RGB only: the test image's alpha is opaque and not part of the check.
    struct pl_frame image = {
        .num_planes = 1,
        .planes = {{ .texture = src, .components = 3, .component_mapping = {0, 1, 2} }},
        .repr = pl_color_repr_rgb,
        .color = pl_color_space_srgb,
    };
    struct pl_frame target = image;
    target.planes[0].texture = dst;

    // Same size, same colour space: an identity render. No dithering, so the
    // output can be compared with the input exactly.
    struct pl_render_params params = pl_render_default_params;
    params.dither_params = NULL;
    if (!pl_render_image(pl->renderer, &image, &target, &params)) goto done;

    if (!pl_tex_download(gpu, pl_tex_transfer_params(
            .tex = dst, .ptr = out, .row_pitch = (size_t)w * 4)))
        goto done;
    pl_gpu_finish(gpu);
    rc = 0;

done:
    pl_tex_destroy(gpu, &src);
    pl_tex_destroy(gpu, &dst);
    return rc;
}

static void log_err(struct cf_pl *pl, const char *msg) {
    if (pl->log_cb) pl->log_cb(pl->log_ctx, PL_LOG_ERR, msg);
}

static pl_tex unwrap_tex(cf_pl_tex *t) { return (pl_tex)(const void *)t; }
static cf_pl_tex *wrap_tex(pl_tex t) { return (cf_pl_tex *)(void *)t; }

cf_pl_tex *cf_pl_import_dmabuf(cf_pl *pl, int fd, int w, int h, uint32_t fourcc,
                               uint64_t modifier, uint32_t stride, uint32_t offset) {
    pl_gpu gpu = pl->vk->gpu;
    pl_fmt fmt = pl_find_fourcc(gpu, fourcc);
    // libplacebo registers the A variant of formats with a padding byte
    // (ARGB8888, not XRGB8888). The memory layout is the same and a render
    // target's fourth byte goes unused, so fall back to it. DRM's X and A
    // codes differ only in their first character: 'XR24' and 'AR24'.
    if (!fmt && (fourcc & 0xFF) == 'X')
        fmt = pl_find_fourcc(gpu, (fourcc & ~0xFFu) | 'A');
    if (!fmt) {
        log_err(pl, "no libplacebo format for the scanout buffer's DRM fourcc");
        return NULL;
    }
    // libplacebo dup()s the fd for the import and closes only its own copy.
    // Blit target too: a picture placed inside the target leaves bars, which
    // libplacebo clears with a blit, and refuses to without this.
    pl_tex tex = pl_tex_create(gpu, pl_tex_params(
        .w = w, .h = h, .format = fmt, .renderable = true,
        .blit_dst = (fmt->caps & PL_FMT_CAP_BLITTABLE) != 0,
        .import_handle = PL_HANDLE_DMA_BUF,
        .shared_mem = {
            .handle.fd = fd,
            .size = (size_t)stride * (size_t)h,
            .offset = offset,
            .drm_format_mod = modifier,
            // In bytes, despite the name: libplacebo hands it to Vulkan as
            // the plane's rowPitch (gpu_tex.c, v7.360.0). Passing pixels
            // packs several GPU rows into each real one.
            .stride_w = stride,
        },
    ));
    return wrap_tex(tex);
}

cf_pl_tex *cf_pl_upload_rgba8(cf_pl *pl, int w, int h, const uint8_t *rgba) {
    pl_gpu gpu = pl->vk->gpu;
    pl_fmt fmt = pl_find_named_fmt(gpu, "rgba8");
    if (!fmt) {
        log_err(pl, "no rgba8 texture format");
        return NULL;
    }
    pl_tex tex = pl_tex_create(gpu, pl_tex_params(
        .w = w, .h = h, .format = fmt, .sampleable = true, .initial_data = rgba));
    return wrap_tex(tex);
}

void cf_pl_tex_destroy(cf_pl *pl, cf_pl_tex *t) {
    pl_tex tex = unwrap_tex(t);
    pl_tex_destroy(pl->vk->gpu, &tex);
}

int cf_pl_clear_black(cf_pl *pl, cf_pl_tex *dst) {
    // Zero in every channel: black for PQ and SDR alike, full-range RGB.
    pl_tex_clear(pl->vk->gpu, unwrap_tex(dst), (float[4]){0.0f, 0.0f, 0.0f, 1.0f});
    pl_gpu_finish(pl->vk->gpu);
    return 0;
}

int cf_pl_render(cf_pl *pl, cf_pl_tex *src, cf_pl_tex *dst, float shift_x) {
    pl_tex s = unwrap_tex(src), d = unwrap_tex(dst);
    struct pl_frame image = {
        .num_planes = 1,
        .planes = {{
            .texture = s,
            .components = 3,
            .component_mapping = {0, 1, 2},
            // A crop past the edge wraps around instead of smearing it.
            .address_mode = PL_TEX_ADDRESS_REPEAT,
        }},
        .repr = pl_color_repr_rgb,
        .color = pl_color_space_srgb,
        .crop = { shift_x, 0, shift_x + (float)s->params.w, (float)s->params.h },
    };
    struct pl_frame target = {
        .num_planes = 1,
        .planes = {{ .texture = d, .components = 3, .component_mapping = {0, 1, 2} }},
        .repr = pl_color_repr_rgb,
        .color = pl_color_space_srgb,
    };

    struct pl_render_params params = pl_render_default_params;
    params.dither_params = NULL;
    if (!pl_render_image(pl->renderer, &image, &target, &params)) return -1;
    // Before the display reads the buffer, the GPU must be done writing it.
    // Waiting here is the simple way; a fence handed to the flip can replace
    // it later.
    pl_gpu_finish(pl->vk->gpu);
    return 0;
}

struct cf_pl_planes {
    pl_tex luma, chroma;
    pl_tex packed;      /* the frame as uploaded; sized on first use */
    int w, h;
    int depth;          /* of the last unpack: 10 (v210) or 12 (tunnel) */
    bool filled;        /* an unpack has written the planes */
};

static pl_fmt storable_fmt(struct cf_pl *pl, const char *name) {
    pl_fmt fmt = pl_find_named_fmt(pl->vk->gpu, name);
    if (!fmt || !(fmt->caps & PL_FMT_CAP_STORABLE) || !(fmt->caps & PL_FMT_CAP_SAMPLEABLE)) {
        log_err(pl, "a plane format is not both storable and sampleable");
        return NULL;
    }
    return fmt;
}

cf_pl_planes *cf_pl_planes_create(cf_pl *pl, int w, int h) {
    pl_gpu gpu = pl->vk->gpu;
    pl_fmt f_luma = storable_fmt(pl, "r16"), f_chroma = storable_fmt(pl, "rg16");
    if (!f_luma || !f_chroma) return NULL;
    struct cf_pl_planes *p = calloc(1, sizeof *p);
    if (!p) return NULL;
    p->w = w;
    p->h = h;
    // Host readable only so the checks can read them back.
    p->luma = pl_tex_create(gpu, pl_tex_params(
        .w = w, .h = h, .format = f_luma,
        .sampleable = true, .storable = true, .host_readable = true));
    p->chroma = pl_tex_create(gpu, pl_tex_params(
        .w = (w + 1) / 2, .h = h, .format = f_chroma,
        .sampleable = true, .storable = true, .host_readable = true));
    if (!p->luma || !p->chroma) {
        cf_pl_planes_destroy(pl, p);
        return NULL;
    }
    return p;
}

void cf_pl_planes_destroy(cf_pl *pl, cf_pl_planes *p) {
    if (!p) return;
    pl_gpu gpu = pl->vk->gpu;
    pl_tex_destroy(gpu, &p->luma);
    pl_tex_destroy(gpu, &p->chroma);
    pl_tex_destroy(gpu, &p->packed);
    free(p);
}

/* One invocation per v210 group: six pixels in four little-endian 32-bit
 * words, three 10-bit components each (the C++ version's layout, checked
 * against the card there):
 *
 *   word 0:  bits  0-9 Cb0   10-19 Y0    20-29 Cr0
 *   word 1:  bits  0-9 Y1    10-19 Cb1   20-29 Y2
 *   word 2:  bits  0-9 Cr1   10-19 Y3    20-29 Cb2
 *   word 3:  bits  0-9 Y4    10-19 Cr2   20-29 Y5
 *
 * The frame is uploaded as an r32u texture, one texel per word, so the
 * upload honours the card's stride and the shader reads exact integers.
 * Codes are stored shifted left by 6, which a 16-bit unorm holds exactly and
 * libplacebo undoes with bit_shift = 6. The names carry a prefix because
 * libplacebo #defines each one over the whole shader. */
static const char V210_HEADER[] =
    "float cf_unorm(uint code) { return float(code << 6) / 65535.0; }\n";

static const char V210_BODY[] =
    "ivec2 id = ivec2(gl_GlobalInvocationID.xy);\n"
    "if (id.x < cf_groups && id.y < cf_height) {\n"
    "    uint w0 = texelFetch(cf_packed, ivec2(4 * id.x + 0, id.y), 0).r;\n"
    "    uint w1 = texelFetch(cf_packed, ivec2(4 * id.x + 1, id.y), 0).r;\n"
    "    uint w2 = texelFetch(cf_packed, ivec2(4 * id.x + 2, id.y), 0).r;\n"
    "    uint w3 = texelFetch(cf_packed, ivec2(4 * id.x + 3, id.y), 0).r;\n"
    "    uint y[6] = uint[6](w0 >> 10, w1, w1 >> 20, w2 >> 10, w3, w3 >> 20);\n"
    "    uint cb[3] = uint[3](w0, w1 >> 10, w2 >> 20);\n"
    "    uint cr[3] = uint[3](w0 >> 20, w2, w3 >> 10);\n"
    "    for (int i = 0; i < 6; i++) {\n"
    "        int x = 6 * id.x + i;\n"
    "        if (x < cf_width)\n"
    "            imageStore(cf_luma, ivec2(x, id.y), vec4(cf_unorm(y[i] & 0x3FFu), 0.0, 0.0, 1.0));\n"
    "    }\n"
    /* ceil(width / 2) pairs: an odd width's last pixel still has one. */
    "    for (int i = 0; i < 3; i++) {\n"
    "        int x = 3 * id.x + i;\n"
    "        if (x < (cf_width + 1) / 2)\n"
    "            imageStore(cf_chroma, ivec2(x, id.y),\n"
    "                       vec4(cf_unorm(cb[i] & 0x3FFu), cf_unorm(cr[i] & 0x3FFu), 0.0, 1.0));\n"
    "    }\n"
    "}\n";

/* Workgroup width; one row of groups per workgroup row. */
#define V210_LOCAL 64

int cf_pl_unpack_v210(cf_pl *pl, cf_pl_planes *p, const uint8_t *data, size_t row_bytes) {
    pl_gpu gpu = pl->vk->gpu;
    int groups = (p->w + 5) / 6;
    if (row_bytes % 16 || row_bytes < (size_t)groups * 16) {
        log_err(pl, "v210 stride is not a multiple of 16 covering the width");
        return -1;
    }
    pl_fmt f_packed = pl_find_named_fmt(gpu, "r32u");
    if (!f_packed) {
        log_err(pl, "no r32u texture format");
        return -1;
    }
    if (!pl_tex_recreate(gpu, &p->packed, pl_tex_params(
            .w = (int)(row_bytes / 4), .h = p->h, .format = f_packed,
            .sampleable = true, .host_writable = true)))
        return -1;
    if (!pl_tex_upload(gpu, pl_tex_transfer_params(
            .tex = p->packed, .ptr = (void *)data, .row_pitch = row_bytes)))
        return -1;

    pl_shader sh = pl_dispatch_begin(pl->dispatch);
    bool ok = pl_shader_custom(sh, &(struct pl_custom_shader) {
        .description = "v210 unpack",
        .header = V210_HEADER,
        .body = V210_BODY,
        .input = PL_SHADER_SIG_NONE,
        .output = PL_SHADER_SIG_NONE,
        .compute = true,
        .compute_group_size = {V210_LOCAL, 1},
        .descriptors = (struct pl_shader_desc[]) {
            {
                .desc = { .name = "cf_packed", .type = PL_DESC_SAMPLED_TEX },
                // Integer texels: fetched, never filtered.
                .binding = { .object = p->packed, .sample_mode = PL_TEX_SAMPLE_NEAREST },
            },
            {
                .desc = { .name = "cf_luma", .type = PL_DESC_STORAGE_IMG,
                          .access = PL_DESC_ACCESS_WRITEONLY },
                .binding.object = p->luma,
            },
            {
                .desc = { .name = "cf_chroma", .type = PL_DESC_STORAGE_IMG,
                          .access = PL_DESC_ACCESS_WRITEONLY },
                .binding.object = p->chroma,
            },
        },
        .num_descriptors = 3,
        .variables = (struct pl_shader_var[]) {
            { .var = pl_var_int("cf_width"),  .data = &p->w },
            { .var = pl_var_int("cf_height"), .data = &p->h },
            { .var = pl_var_int("cf_groups"), .data = &groups },
        },
        .num_variables = 3,
    });
    if (!ok) {
        pl_dispatch_abort(pl->dispatch, &sh);
        log_err(pl, "v210 unpack shader rejected");
        return -1;
    }
    if (!pl_dispatch_compute(pl->dispatch, pl_dispatch_compute_params(
            .shader = &sh,
            .dispatch_size = {(groups + V210_LOCAL - 1) / V210_LOCAL, p->h, 1})))
        return -1;
    p->depth = 10;
    p->filled = true;
    return 0;
}

int cf_pl_planes_download(cf_pl *pl, cf_pl_planes *p, uint16_t *luma, uint16_t *chroma) {
    pl_gpu gpu = pl->vk->gpu;
    // Until then the textures hold whatever the allocation did.
    if (!p->filled) {
        log_err(pl, "planes read before any frame was unpacked into them");
        return -1;
    }
    if (!pl_tex_download(gpu, pl_tex_transfer_params(
            .tex = p->luma, .ptr = luma, .row_pitch = (size_t)p->w * 2)))
        return -1;
    if (!pl_tex_download(gpu, pl_tex_transfer_params(
            .tex = p->chroma, .ptr = chroma, .row_pitch = (size_t)((p->w + 1) / 2) * 4)))
        return -1;
    pl_gpu_finish(gpu);
    return 0;
}

static enum pl_color_primaries primaries(int p) {
    return p == CF_PL_PRIM_BT2020 ? PL_COLOR_PRIM_BT_2020 : PL_COLOR_PRIM_BT_709;
}

/* Where in the target the picture goes, as x0, y0, x1, y1 in pixels, or the
 * whole of it for NULL. libplacebo clears the rest to black. */
static void set_place(struct pl_frame *target, const float *place) {
    if (place)
        target->crop = (pl_rect2df) { place[0], place[1], place[2], place[3] };
}

/* Which part of the source is drawn, as x0, y0, x1, y1 in source pixels, or
 * the whole frame for NULL: the bars cropped out, as a crop rect and nothing
 * else, since a geometry change that commits is a bug. */
static void set_source(struct pl_frame *image, const float *src) {
    if (src)
        image->crop = (pl_rect2df) { src[0], src[1], src[2], src[3] };
}

static enum pl_color_transfer transfer(int t) {
    switch (t) {
    case CF_PL_TRC_PQ:  return PL_COLOR_TRC_PQ;
    case CF_PL_TRC_HLG: return PL_COLOR_TRC_HLG;
    default:            return PL_COLOR_TRC_BT_1886;
    }
}

static const struct pl_tone_map_function *curve(int c) {
    switch (c) {
    case CF_PL_CURVE_HABLE:      return &pl_tone_map_hable;
    case CF_PL_CURVE_SPLINE:     return &pl_tone_map_spline;
    case CF_PL_CURVE_BT2390:     return &pl_tone_map_bt2390;
    case CF_PL_CURVE_BT2446A:    return &pl_tone_map_bt2446a;
    case CF_PL_CURVE_ST2094_10:  return &pl_tone_map_st2094_10;
    case CF_PL_CURVE_ST2094_40:  return &pl_tone_map_st2094_40;
    case CF_PL_CURVE_MOBIUS:     return &pl_tone_map_mobius;
    case CF_PL_CURVE_REINHARD:   return &pl_tone_map_reinhard;
    default:                     return &pl_tone_map_clip;
    }
}

/* Applies a mapping to a render about to happen. The target's black is
 * libplacebo's HDR black, not SDR's default 1000:1: BT.1886 would otherwise
 * lift the shadows to emulate a display far greyer than a projector in a dark
 * room. Contrast recovery is off: it sharpens, and this path is to be judged
 * against Dolby's own renditions, which do not. */
static void apply_map(struct pl_frame *image, struct pl_frame *target,
                      struct pl_render_params *params, struct pl_color_map_params *cmp,
                      const cf_pl_map *map) {
    if (!map) return;
    if (map->src_peak > 0) image->color.hdr.max_luma = map->src_peak;
    if (map->dst_peak > 0) target->color.hdr.max_luma = map->dst_peak;
    target->color.hdr.min_luma = PL_COLOR_HDR_BLACK;
    *cmp = pl_color_map_default_params;
    cmp->tone_mapping_function = curve(map->curve);
    cmp->contrast_recovery = 0.0f;
    params->color_map_params = cmp;
    // Only here, where there is a mapping to make: the callers turn it off for
    // renders between identical spaces, where it would invent one.
    if (map->peak_detect) params->peak_detect_params = &pl_peak_detect_default_params;
}

int cf_pl_render_ycbcr(cf_pl *pl, cf_pl_planes *src, int matrix, int prim, int trc,
                       cf_pl_tex *dst, int dst_prim, int dst_trc, const float *place,
                       const cf_pl_map *map, const float *src_crop) {
    if (!src->filled) {
        log_err(pl, "planes read before any frame was unpacked into them");
        return -1;
    }
    if (src->depth != 10) {
        log_err(pl, "YCbCr render of planes that do not hold v210");
        return -1;
    }
    struct pl_frame image = {
        .num_planes = 2,
        .planes = {
            { .texture = src->luma, .components = 1, .component_mapping = {PL_CHANNEL_Y} },
            { .texture = src->chroma, .components = 2,
              .component_mapping = {PL_CHANNEL_CB, PL_CHANNEL_CR} },
        },
        .repr = {
            .sys = matrix == CF_PL_MATRIX_BT2020_NC ? PL_COLOR_SYSTEM_BT_2020_NC
                                                    : PL_COLOR_SYSTEM_BT_709,
            .levels = PL_COLOR_LEVELS_LIMITED,
            .bits = { .sample_depth = 16, .color_depth = 10, .bit_shift = 6 },
        },
        .color = { .primaries = primaries(prim), .transfer = transfer(trc) },
    };
    // HDMI 4:2:2 is co-sited: each chroma sample sits on an even luma pixel.
    // A plane built by hand has shift_x 0, which libplacebo reads as centred
    // between two luma pixels.
    pl_frame_set_chroma_location(&image, PL_CHROMA_LEFT);

    pl_tex d = unwrap_tex(dst);
    struct pl_frame target = {
        .num_planes = 1,
        .planes = {{ .texture = d, .components = 3, .component_mapping = {0, 1, 2} }},
        .repr = pl_color_repr_rgb,
        .color = { .primaries = primaries(dst_prim), .transfer = transfer(dst_trc) },
    };
    set_place(&target, place);
    set_source(&image, src_crop);

    struct pl_render_params params = pl_render_default_params;
    params.dither_params = NULL;
    // Peak detection would measure the frame and change the source's HDR
    // metadata, which turns a render between identical colour spaces into a
    // tone mapping. Passthrough wants none.
    params.peak_detect_params = NULL;
    struct pl_color_map_params cmp;
    apply_map(&image, &target, &params, &cmp, map);
    if (!pl_render_image(pl->renderer, &image, &target, &params)) return -1;
    pl_gpu_finish(pl->vk->gpu);
    return 0;
}

cf_pl_tex *cf_pl_target_rgba16(cf_pl *pl, int w, int h) {
    pl_fmt fmt = pl_find_named_fmt(pl->vk->gpu, "rgba16");
    if (!fmt) {
        log_err(pl, "no rgba16 texture format");
        return NULL;
    }
    // Blittable as well: a render that leaves part of the target uncovered
    // clears that part, which libplacebo does as a blit.
    return wrap_tex(pl_tex_create(pl->vk->gpu, pl_tex_params(
        .w = w, .h = h, .format = fmt, .renderable = true, .host_readable = true,
        .blit_dst = (fmt->caps & PL_FMT_CAP_BLITTABLE) != 0)));
}

int cf_pl_download_rgba16(cf_pl *pl, cf_pl_tex *t, uint16_t *out) {
    pl_tex tex = unwrap_tex(t);
    if (!pl_tex_download(pl->vk->gpu, pl_tex_transfer_params(
            .tex = tex, .ptr = out, .row_pitch = (size_t)tex->params.w * 8)))
        return -1;
    pl_gpu_finish(pl->vk->gpu);
    return 0;
}

struct cf_pl_rgb {
    pl_tex rgb;
    pl_tex packed;      /* the frame as uploaded; sized on first use */
    int w, h;
    int depth;          /* of the last unpack: 10 or 8 */
    bool filled;        /* an unpack has written the texture */
};

cf_pl_rgb *cf_pl_rgb_create(cf_pl *pl, int w, int h) {
    pl_fmt fmt = storable_fmt(pl, "rgba16");
    if (!fmt) return NULL;
    struct cf_pl_rgb *r = calloc(1, sizeof *r);
    if (!r) return NULL;
    r->w = w;
    r->h = h;
    r->depth = 10;
    r->rgb = pl_tex_create(pl->vk->gpu, pl_tex_params(
        .w = w, .h = h, .format = fmt,
        .sampleable = true, .storable = true, .host_readable = true));
    if (!r->rgb) {
        cf_pl_rgb_destroy(pl, r);
        return NULL;
    }
    return r;
}

void cf_pl_rgb_destroy(cf_pl *pl, cf_pl_rgb *r) {
    if (!r) return;
    pl_tex_destroy(pl->vk->gpu, &r->rgb);
    pl_tex_destroy(pl->vk->gpu, &r->packed);
    free(r);
}

/* One invocation per pixel, one 32-bit word each. The upload is an r32u
 * texture, so the shader sees each word as the CPU reads it little-endian:
 * r210's big-endian word is byte-swapped back, and ARGB8's bytes A, R, G, B
 * are the word's low byte first. */
static const char RGB_BODY[] =
    "ivec2 id = ivec2(gl_GlobalInvocationID.xy);\n"
    "if (id.x < cf_width && id.y < cf_height) {\n"
    "    uint w = texelFetch(cf_packed, id, 0).r;\n"
    "    vec3 c;\n"
    "    if (cf_format == 0) {\n"
    "        w = (w >> 24) | ((w >> 8) & 0xFF00u) | ((w << 8) & 0xFF0000u) | (w << 24);\n"
    "        c = vec3((uvec3(w >> 20, w >> 10, w) & 0x3FFu) << 6) / 65535.0;\n"
    "    } else {\n"
    "        c = vec3((uvec3(w >> 8, w >> 16, w >> 24) & 0xFFu) << 8) / 65535.0;\n"
    "    }\n"
    "    imageStore(cf_rgb, id, vec4(c, 1.0));\n"
    "}\n";

#define RGB_LOCAL_X 32
#define RGB_LOCAL_Y 8

int cf_pl_unpack_rgb(cf_pl *pl, cf_pl_rgb *r, const uint8_t *data, size_t row_bytes, int format) {
    pl_gpu gpu = pl->vk->gpu;
    if (format != CF_PL_R210 && format != CF_PL_ARGB8) {
        log_err(pl, "unknown RGB capture format");
        return -1;
    }
    if (row_bytes % 4 || row_bytes < (size_t)r->w * 4) {
        log_err(pl, "RGB stride is not whole words covering the width");
        return -1;
    }
    pl_fmt f_packed = pl_find_named_fmt(gpu, "r32u");
    if (!f_packed) {
        log_err(pl, "no r32u texture format");
        return -1;
    }
    if (!pl_tex_recreate(gpu, &r->packed, pl_tex_params(
            .w = (int)(row_bytes / 4), .h = r->h, .format = f_packed,
            .sampleable = true, .host_writable = true)))
        return -1;
    if (!pl_tex_upload(gpu, pl_tex_transfer_params(
            .tex = r->packed, .ptr = (void *)data, .row_pitch = row_bytes)))
        return -1;

    pl_shader sh = pl_dispatch_begin(pl->dispatch);
    bool ok = pl_shader_custom(sh, &(struct pl_custom_shader) {
        .description = format == CF_PL_R210 ? "r210 unpack" : "ARGB8 unpack",
        .body = RGB_BODY,
        .input = PL_SHADER_SIG_NONE,
        .output = PL_SHADER_SIG_NONE,
        .compute = true,
        .compute_group_size = {RGB_LOCAL_X, RGB_LOCAL_Y},
        .descriptors = (struct pl_shader_desc[]) {
            {
                .desc = { .name = "cf_packed", .type = PL_DESC_SAMPLED_TEX },
                .binding = { .object = r->packed, .sample_mode = PL_TEX_SAMPLE_NEAREST },
            },
            {
                .desc = { .name = "cf_rgb", .type = PL_DESC_STORAGE_IMG,
                          .access = PL_DESC_ACCESS_WRITEONLY },
                .binding.object = r->rgb,
            },
        },
        .num_descriptors = 2,
        .variables = (struct pl_shader_var[]) {
            { .var = pl_var_int("cf_width"),  .data = &r->w },
            { .var = pl_var_int("cf_height"), .data = &r->h },
            { .var = pl_var_int("cf_format"), .data = &format },
        },
        .num_variables = 3,
    });
    if (!ok) {
        pl_dispatch_abort(pl->dispatch, &sh);
        log_err(pl, "RGB unpack shader rejected");
        return -1;
    }
    if (!pl_dispatch_compute(pl->dispatch, pl_dispatch_compute_params(
            .shader = &sh,
            .dispatch_size = {(r->w + RGB_LOCAL_X - 1) / RGB_LOCAL_X,
                              (r->h + RGB_LOCAL_Y - 1) / RGB_LOCAL_Y, 1})))
        return -1;
    r->depth = format == CF_PL_R210 ? 10 : 8;
    r->filled = true;
    return 0;
}

int cf_pl_rgb_download(cf_pl *pl, cf_pl_rgb *r, uint16_t *out) {
    // Until then the texture holds whatever the allocation did.
    if (!r->filled) {
        log_err(pl, "RGB read before any frame was unpacked into it");
        return -1;
    }
    if (!pl_tex_download(pl->vk->gpu, pl_tex_transfer_params(
            .tex = r->rgb, .ptr = out, .row_pitch = (size_t)r->w * 8)))
        return -1;
    pl_gpu_finish(pl->vk->gpu);
    return 0;
}

int cf_pl_render_rgb(cf_pl *pl, cf_pl_rgb *src, int levels, int prim, int trc,
                     cf_pl_tex *dst, int dst_prim, int dst_trc, const float *place,
                     const float *src_crop) {
    if (!src->filled) {
        log_err(pl, "RGB read before any frame was unpacked into it");
        return -1;
    }
    struct pl_frame image = {
        .num_planes = 1,
        .planes = {{ .texture = src->rgb, .components = 3, .component_mapping = {0, 1, 2} }},
        .repr = {
            .sys = PL_COLOR_SYSTEM_RGB,
            .levels = levels == CF_PL_LEVELS_LIMITED ? PL_COLOR_LEVELS_LIMITED
                                                     : PL_COLOR_LEVELS_FULL,
            .bits = { .sample_depth = 16, .color_depth = src->depth,
                      .bit_shift = 16 - src->depth },
        },
        .color = { .primaries = primaries(prim), .transfer = transfer(trc) },
    };
    pl_tex d = unwrap_tex(dst);
    struct pl_frame target = {
        .num_planes = 1,
        .planes = {{ .texture = d, .components = 3, .component_mapping = {0, 1, 2} }},
        .repr = pl_color_repr_rgb,
        .color = { .primaries = primaries(dst_prim), .transfer = transfer(dst_trc) },
    };
    set_place(&target, place);
    set_source(&image, src_crop);

    struct pl_render_params params = pl_render_default_params;
    params.dither_params = NULL;
    // As in cf_pl_render_ycbcr: no tone mapping between identical spaces.
    params.peak_detect_params = NULL;
    if (!pl_render_image(pl->renderer, &image, &target, &params)) return -1;
    pl_gpu_finish(pl->vk->gpu);
    return 0;
}

/* One invocation per pair of pixels: two big-endian r210 words, each 8-bit
 * code in the top of its 10 bits. Per pixel, as HDMI packs 12-bit 4:2:2:
 * G = Y[11:4], R = C[11:4], B = C[3:0] << 4 | Y[3:0]; C is Cb on the even
 * pixel and Cr on the odd one (ETSI GS CCM 001 clause 6.4.1). Codes are
 * stored shifted left by 4. */
static const char TUNNEL_HEADER[] =
    "uvec3 cf_rgb8(uint w) {\n"
    "    w = (w >> 24) | ((w >> 8) & 0xFF00u) | ((w << 8) & 0xFF0000u) | (w << 24);\n"
    "    return uvec3(w >> 22, w >> 12, w >> 2) & 0xFFu;\n"
    "}\n"
    "float cf_unorm12(uint code) { return float(code << 4) / 65535.0; }\n";

static const char TUNNEL_BODY[] =
    "ivec2 id = ivec2(gl_GlobalInvocationID.xy);\n"
    "if (id.x < cf_pairs && id.y < cf_height) {\n"
    "    uvec3 a = cf_rgb8(texelFetch(cf_packed, ivec2(2 * id.x, id.y), 0).r);\n"
    "    uvec3 b = cf_rgb8(texelFetch(cf_packed, ivec2(2 * id.x + 1, id.y), 0).r);\n"
    "    uint ya = a.g << 4 | (a.b & 0xFu), yb = b.g << 4 | (b.b & 0xFu);\n"
    "    uint cb = a.r << 4 | a.b >> 4, cr = b.r << 4 | b.b >> 4;\n"
    "    imageStore(cf_luma, ivec2(2 * id.x, id.y), vec4(cf_unorm12(ya), 0.0, 0.0, 1.0));\n"
    "    imageStore(cf_luma, ivec2(2 * id.x + 1, id.y), vec4(cf_unorm12(yb), 0.0, 0.0, 1.0));\n"
    "    imageStore(cf_chroma, ivec2(id.x, id.y), vec4(cf_unorm12(cb), cf_unorm12(cr), 0.0, 1.0));\n"
    "}\n";

int cf_pl_unpack_tunnel(cf_pl *pl, cf_pl_planes *p, const uint8_t *data, size_t row_bytes) {
    pl_gpu gpu = pl->vk->gpu;
    if (p->w % 2) {
        log_err(pl, "a tunnel frame has an even width");
        return -1;
    }
    if (row_bytes % 4 || row_bytes < (size_t)p->w * 4) {
        log_err(pl, "r210 stride is not whole words covering the width");
        return -1;
    }
    pl_fmt f_packed = pl_find_named_fmt(gpu, "r32u");
    if (!f_packed) {
        log_err(pl, "no r32u texture format");
        return -1;
    }
    if (!pl_tex_recreate(gpu, &p->packed, pl_tex_params(
            .w = (int)(row_bytes / 4), .h = p->h, .format = f_packed,
            .sampleable = true, .host_writable = true)))
        return -1;
    if (!pl_tex_upload(gpu, pl_tex_transfer_params(
            .tex = p->packed, .ptr = (void *)data, .row_pitch = row_bytes)))
        return -1;

    int pairs = p->w / 2;
    pl_shader sh = pl_dispatch_begin(pl->dispatch);
    bool ok = pl_shader_custom(sh, &(struct pl_custom_shader) {
        .description = "Dolby Vision tunnel unpack",
        .header = TUNNEL_HEADER,
        .body = TUNNEL_BODY,
        .input = PL_SHADER_SIG_NONE,
        .output = PL_SHADER_SIG_NONE,
        .compute = true,
        .compute_group_size = {RGB_LOCAL_X, RGB_LOCAL_Y},
        .descriptors = (struct pl_shader_desc[]) {
            {
                .desc = { .name = "cf_packed", .type = PL_DESC_SAMPLED_TEX },
                .binding = { .object = p->packed, .sample_mode = PL_TEX_SAMPLE_NEAREST },
            },
            {
                .desc = { .name = "cf_luma", .type = PL_DESC_STORAGE_IMG,
                          .access = PL_DESC_ACCESS_WRITEONLY },
                .binding.object = p->luma,
            },
            {
                .desc = { .name = "cf_chroma", .type = PL_DESC_STORAGE_IMG,
                          .access = PL_DESC_ACCESS_WRITEONLY },
                .binding.object = p->chroma,
            },
        },
        .num_descriptors = 3,
        .variables = (struct pl_shader_var[]) {
            { .var = pl_var_int("cf_pairs"),  .data = &pairs },
            { .var = pl_var_int("cf_height"), .data = &p->h },
        },
        .num_variables = 2,
    });
    if (!ok) {
        pl_dispatch_abort(pl->dispatch, &sh);
        log_err(pl, "tunnel unpack shader rejected");
        return -1;
    }
    if (!pl_dispatch_compute(pl->dispatch, pl_dispatch_compute_params(
            .shader = &sh,
            .dispatch_size = {(pairs + RGB_LOCAL_X - 1) / RGB_LOCAL_X,
                              (p->h + RGB_LOCAL_Y - 1) / RGB_LOCAL_Y, 1})))
        return -1;
    p->depth = 12;
    p->filled = true;
    return 0;
}

int cf_pl_render_dovi(cf_pl *pl, cf_pl_planes *src, const cf_pl_dovi *d,
                      cf_pl_tex *dst, int dst_prim, int dst_trc, const float *place,
                      const cf_pl_map *map, const float *src_crop) {
    if (!src->filled) {
        log_err(pl, "planes read before any frame was unpacked into them");
        return -1;
    }
    if (src->depth != 12) {
        log_err(pl, "Dolby Vision render of planes that do not hold a tunnel");
        return -1;
    }
    struct pl_dovi_metadata meta = {0};
    for (int i = 0; i < 3; i++) {
        meta.nonlinear_offset[i] = d->offset[i];
        for (int j = 0; j < 3; j++) {
            meta.nonlinear.m[i][j] = d->ycc_to_lms[3 * i + j];
            meta.linear.m[i][j] = d->linear[3 * i + j];
        }
        // The player has composed the picture already: reshaping is the
        // identity, one polynomial piece y = x over the whole range.
        meta.comp[i].num_pivots = 2;
        meta.comp[i].pivots[0] = 0.0f;
        meta.comp[i].pivots[1] = 1.0f;
        meta.comp[i].method[0] = 0;
        meta.comp[i].poly_coeffs[0][1] = 1.0f;
    }

    struct pl_frame image = {
        .num_planes = 2,
        .planes = {
            { .texture = src->luma, .components = 1, .component_mapping = {PL_CHANNEL_Y} },
            { .texture = src->chroma, .components = 2,
              .component_mapping = {PL_CHANNEL_CB, PL_CHANNEL_CR} },
        },
        .repr = {
            .sys = PL_COLOR_SYSTEM_DOLBYVISION,
            .levels = PL_COLOR_LEVELS_FULL,
            // Described as 16-bit, not 12 shifted by 4: libplacebo scales
            // the offsets by 2^n / (2^n - 1) for the texture's n bits, and
            // only with n the texture's own 16 does chroma code 2048 land
            // exactly on the 0.5 centre. The range is in the matrix.
            .bits = { .sample_depth = 16, .color_depth = 16, .bit_shift = 0 },
            .dovi = &meta,
        },
        // Dolby Vision's output is BT.2020 PQ, whatever the card reports
        // (a tunnel arrives labelled SDR).
        .color = { .primaries = PL_COLOR_PRIM_BT_2020, .transfer = PL_COLOR_TRC_PQ },
    };
    // Co-sited, as clause 6.4.1 pairs Cb[0] and Cr[0] with Y[0].
    pl_frame_set_chroma_location(&image, PL_CHROMA_LEFT);

    pl_tex t = unwrap_tex(dst);
    struct pl_frame target = {
        .num_planes = 1,
        .planes = {{ .texture = t, .components = 3, .component_mapping = {0, 1, 2} }},
        .repr = pl_color_repr_rgb,
        .color = { .primaries = primaries(dst_prim), .transfer = transfer(dst_trc) },
    };
    set_place(&target, place);
    set_source(&image, src_crop);

    struct pl_render_params params = pl_render_default_params;
    params.dither_params = NULL;
    // As in cf_pl_render_ycbcr: no tone mapping between identical spaces.
    params.peak_detect_params = NULL;
    struct pl_color_map_params cmp;
    apply_map(&image, &target, &params, &cmp, map);
    if (!pl_render_image(pl->renderer, &image, &target, &params)) return -1;
    pl_gpu_finish(pl->vk->gpu);
    return 0;
}
