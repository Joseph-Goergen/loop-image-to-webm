/*
 * Minimal C shim over the libvpx VP9 encoder.
 *
 * Keeping all libvpx structs (vpx_codec_enc_cfg_t, vpx_image_t, packet unions)
 * on the C side means the Rust FFI surface is a handful of plain functions and
 * is ABI-safe on every target (LP64 and LLP64 alike) without bindgen.
 */
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

#include "vpx/vpx_encoder.h"
#include "vpx/vp8cx.h"

typedef struct lw_enc {
  vpx_codec_ctx_t codec;
  vpx_image_t img;
  int width, height;
  vpx_codec_iter_t iter;
  int iter_valid;
} lw_enc;

typedef struct lw_enc_params {
  int width;
  int height;
  int fps_num;
  int fps_den;
  int crf;          /* 0..63, lower = better */
  int cpu_used;     /* 0..8, higher = faster */
  int threads;
  int kf_max_dist;
} lw_enc_params;

const char *lw_vpx_version(void) { return vpx_codec_version_str(); }

const char *lw_enc_error(lw_enc *e) {
  if (!e) return "null encoder";
  const char *detail = vpx_codec_error_detail(&e->codec);
  return detail ? detail : vpx_codec_error(&e->codec);
}

static int ilog2_floor(int v) {
  int r = 0;
  while (v > 1) { v >>= 1; r++; }
  return r;
}

/* Returns NULL on failure; *err receives a static error string. */
lw_enc *lw_enc_new(const lw_enc_params *p, const char **err) {
  vpx_codec_iface_t *iface = vpx_codec_vp9_cx();
  vpx_codec_enc_cfg_t cfg;
  lw_enc *e;

  if (vpx_codec_enc_config_default(iface, &cfg, 0) != VPX_CODEC_OK) {
    *err = "vpx_codec_enc_config_default failed";
    return NULL;
  }
  cfg.g_w = (unsigned)p->width;
  cfg.g_h = (unsigned)p->height;
  cfg.g_timebase.num = p->fps_den;
  cfg.g_timebase.den = p->fps_num;
  cfg.g_threads = (unsigned)(p->threads > 0 ? p->threads : 1);
  cfg.g_lag_in_frames = 25;
  cfg.g_pass = VPX_RC_ONE_PASS;
  cfg.g_error_resilient = 0;
  cfg.rc_end_usage = VPX_Q; /* constant quality, like ffmpeg -crf N -b:v 0 */
  cfg.rc_min_quantizer = 0;
  cfg.rc_max_quantizer = 63;
  cfg.kf_mode = VPX_KF_AUTO;
  cfg.kf_min_dist = 0;
  cfg.kf_max_dist = (unsigned)(p->kf_max_dist > 0 ? p->kf_max_dist : 240);

  e = (lw_enc *)calloc(1, sizeof(lw_enc));
  if (!e) { *err = "out of memory"; return NULL; }
  e->width = p->width;
  e->height = p->height;

  if (vpx_codec_enc_init(&e->codec, iface, &cfg, 0) != VPX_CODEC_OK) {
    *err = vpx_codec_error(&e->codec);
    free(e);
    return NULL;
  }

  vpx_codec_control(&e->codec, VP8E_SET_CQ_LEVEL, (unsigned)p->crf);
  vpx_codec_control(&e->codec, VP8E_SET_CPUUSED, p->cpu_used);
  vpx_codec_control(&e->codec, VP9E_SET_ROW_MT, 1);
  {
    /* Tile columns: as many as the width allows (min tile width 256px), max 2^4. */
    int tiles = ilog2_floor(p->width / 256);
    if (tiles > 4) tiles = 4;
    if (tiles < 0) tiles = 0;
    vpx_codec_control(&e->codec, VP9E_SET_TILE_COLUMNS, tiles);
  }
  vpx_codec_control(&e->codec, VP8E_SET_ENABLEAUTOALTREF, 1);
  vpx_codec_control(&e->codec, VP9E_SET_AQ_MODE, 0);
  vpx_codec_control(&e->codec, VP9E_SET_COLOR_SPACE, VPX_CS_BT_709);
  vpx_codec_control(&e->codec, VP9E_SET_COLOR_RANGE, VPX_CR_STUDIO_RANGE);

  if (!vpx_img_alloc(&e->img, VPX_IMG_FMT_I420, (unsigned)p->width, (unsigned)p->height, 32)) {
    *err = "vpx_img_alloc failed";
    vpx_codec_destroy(&e->codec);
    free(e);
    return NULL;
  }
  return e;
}

static void copy_plane(uint8_t *dst, int dst_stride, const uint8_t *src, int src_stride, int w, int h) {
  int y;
  for (y = 0; y < h; y++) memcpy(dst + (size_t)y * dst_stride, src + (size_t)y * src_stride, (size_t)w);
}

/*
 * Encode one I420 frame (tightly packed planes: Y w*h, U and V ((w+1)/2)*((h+1)/2)).
 * Pass y == NULL to flush. Returns 0 on success.
 */
int lw_enc_encode(lw_enc *e, const uint8_t *y, const uint8_t *u, const uint8_t *v,
                  int64_t pts, int force_key) {
  vpx_codec_err_t res;
  e->iter = NULL;
  e->iter_valid = 1;
  if (y) {
    int cw = (e->width + 1) / 2, ch = (e->height + 1) / 2;
    copy_plane(e->img.planes[VPX_PLANE_Y], e->img.stride[VPX_PLANE_Y], y, e->width, e->width, e->height);
    copy_plane(e->img.planes[VPX_PLANE_U], e->img.stride[VPX_PLANE_U], u, cw, cw, ch);
    copy_plane(e->img.planes[VPX_PLANE_V], e->img.stride[VPX_PLANE_V], v, cw, cw, ch);
    res = vpx_codec_encode(&e->codec, &e->img, (vpx_codec_pts_t)pts, 1,
                           force_key ? VPX_EFLAG_FORCE_KF : 0, VPX_DL_GOOD_QUALITY);
  } else {
    res = vpx_codec_encode(&e->codec, NULL, 0, 1, 0, VPX_DL_GOOD_QUALITY);
  }
  return res == VPX_CODEC_OK ? 0 : -1;
}

/*
 * Fetch the next compressed frame produced by the last lw_enc_encode call.
 * Returns 1 if a packet was written to the out-params, 0 when there are no more.
 * The data pointer is valid until the next lw_enc_encode call.
 */
int lw_enc_next_packet(lw_enc *e, const uint8_t **data, size_t *size, int64_t *pts, int *is_key) {
  const vpx_codec_cx_pkt_t *pkt;
  if (!e->iter_valid) return 0;
  while ((pkt = vpx_codec_get_cx_data(&e->codec, &e->iter)) != NULL) {
    if (pkt->kind == VPX_CODEC_CX_FRAME_PKT) {
      *data = (const uint8_t *)pkt->data.frame.buf;
      *size = pkt->data.frame.sz;
      *pts = (int64_t)pkt->data.frame.pts;
      *is_key = (pkt->data.frame.flags & VPX_FRAME_IS_KEY) ? 1 : 0;
      return 1;
    }
  }
  return 0;
}

void lw_enc_free(lw_enc *e) {
  if (!e) return;
  vpx_img_free(&e->img);
  vpx_codec_destroy(&e->codec);
  free(e);
}
