#include "placebo.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <libplacebo/config.h>
#include <libplacebo/log.h>
#include <libplacebo/renderer.h>
#include <libplacebo/vulkan.h>

/* The API the shim is written against. A different libplacebo fails the build
 * here rather than misbehaving at run time: update the shim, then this. */
_Static_assert(PL_API_VER == 360, "shim written for libplacebo API 360");

struct cf_pl {
    pl_log log;
    pl_vulkan vk;
    pl_renderer renderer;
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
    return pl;
}

void cf_pl_destroy(cf_pl *pl) {
    if (!pl) return;
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
    pl_tex tex = pl_tex_create(gpu, pl_tex_params(
        .w = w, .h = h, .format = fmt, .renderable = true,
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

int cf_pl_render(cf_pl *pl, cf_pl_tex *src, cf_pl_tex *dst) {
    struct pl_frame image = {
        .num_planes = 1,
        .planes = {{ .texture = unwrap_tex(src), .components = 3, .component_mapping = {0, 1, 2} }},
        .repr = pl_color_repr_rgb,
        .color = pl_color_space_srgb,
    };
    struct pl_frame target = image;
    target.planes[0].texture = unwrap_tex(dst);

    struct pl_render_params params = pl_render_default_params;
    params.dither_params = NULL;
    if (!pl_render_image(pl->renderer, &image, &target, &params)) return -1;
    // Before the display reads the buffer, the GPU must be done writing it.
    // Waiting here is the simple way; a fence handed to the flip can replace
    // it later.
    pl_gpu_finish(pl->vk->gpu);
    return 0;
}
