#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "EbSvtAv1.h"
#include "EbSvtAv1Enc.h"

typedef struct MpSvtAv1Encoder {
    EbComponentType* handle;
    EbSvtAv1EncConfiguration cfg;
} MpSvtAv1Encoder;

typedef struct MpSvtAv1Packet {
    uint8_t* data;
    uint32_t len;
    int64_t pts;
    uint32_t flags;
    uint32_t pic_type;
} MpSvtAv1Packet;

static int map_error(EbErrorType err) {
    return err == EB_ErrorNone ? 0 : (int)err;
}

void* mp_svt_av1_encoder_create(uint32_t width,
                                uint32_t height,
                                uint32_t fps_num,
                                uint32_t fps_den,
                                uint32_t bitrate,
                                int32_t keyint,
                                int32_t enc_mode) {
    MpSvtAv1Encoder* enc = (MpSvtAv1Encoder*)calloc(1, sizeof(MpSvtAv1Encoder));
    if (!enc) {
        return NULL;
    }

    EbErrorType err = svt_av1_enc_init_handle(&enc->handle, &enc->cfg);
    if (err != EB_ErrorNone || !enc->handle) {
        free(enc);
        return NULL;
    }

    enc->cfg.enc_mode = (int8_t)enc_mode;
    enc->cfg.source_width = width;
    enc->cfg.source_height = height;
    enc->cfg.encoder_bit_depth = 8;
    enc->cfg.encoder_color_format = EB_YUV420;
    enc->cfg.profile = MAIN_PROFILE;

    enc->cfg.frame_rate_numerator = fps_num ? fps_num : 30;
    enc->cfg.frame_rate_denominator = fps_den ? fps_den : 1;
    enc->cfg.rate_control_mode = SVT_AV1_RC_MODE_VBR;
    enc->cfg.target_bit_rate = bitrate ? bitrate : 2 * 1000 * 1000;
    enc->cfg.intra_period_length = keyint;
    enc->cfg.look_ahead_distance = 0;

    err = svt_av1_enc_set_parameter(enc->handle, &enc->cfg);
    if (err != EB_ErrorNone) {
        svt_av1_enc_deinit_handle(enc->handle);
        free(enc);
        return NULL;
    }

    err = svt_av1_enc_init(enc->handle);
    if (err != EB_ErrorNone) {
        svt_av1_enc_deinit_handle(enc->handle);
        free(enc);
        return NULL;
    }

    return enc;
}

int mp_svt_av1_encoder_send_i420(void* encoder,
                                 const uint8_t* y,
                                 uint32_t y_stride,
                                 const uint8_t* u,
                                 uint32_t u_stride,
                                 const uint8_t* v,
                                 uint32_t v_stride,
                                 int64_t pts) {
    if (!encoder || !y || !u || !v) {
        return -1;
    }

    EbSvtIOFormat io = {
        .luma = (uint8_t*)y,
        .cb = (uint8_t*)u,
        .cr = (uint8_t*)v,
        .y_stride = y_stride,
        .cb_stride = u_stride,
        .cr_stride = v_stride,
    };

    EbBufferHeaderType in = {0};
    in.size = sizeof(EbBufferHeaderType);
    in.p_buffer = (uint8_t*)&io;
    in.n_filled_len = y_stride;
    in.pts = pts;
    in.pic_type = EB_AV1_INVALID_PICTURE;
    in.flags = 0;

    MpSvtAv1Encoder* enc = (MpSvtAv1Encoder*)encoder;
    return map_error(svt_av1_enc_send_picture(enc->handle, &in));
}

int mp_svt_av1_encoder_send_eos(void* encoder) {
    if (!encoder) {
        return -1;
    }
    EbBufferHeaderType eos = {0};
    eos.size = sizeof(EbBufferHeaderType);
    eos.flags = EB_BUFFERFLAG_EOS;
    eos.pic_type = EB_AV1_INVALID_PICTURE;
    MpSvtAv1Encoder* enc = (MpSvtAv1Encoder*)encoder;
    return map_error(svt_av1_enc_send_picture(enc->handle, &eos));
}

int mp_svt_av1_encoder_get_packet_copy(void* encoder, int pic_send_done, MpSvtAv1Packet* out_packet) {
    if (!encoder || !out_packet) {
        return -1;
    }

    MpSvtAv1Encoder* enc = (MpSvtAv1Encoder*)encoder;
    EbBufferHeaderType* out = NULL;
    EbErrorType err = svt_av1_enc_get_packet(enc->handle, &out, pic_send_done ? 1 : 0);
    if (err == EB_NoErrorEmptyQueue) {
        return 1;
    }
    if (err != EB_ErrorNone || !out) {
        return map_error(err);
    }

    memset(out_packet, 0, sizeof(*out_packet));
    out_packet->len = out->n_filled_len;
    out_packet->pts = out->pts;
    out_packet->flags = out->flags;
    out_packet->pic_type = (uint32_t)out->pic_type;

    if (out->n_filled_len > 0 && out->p_buffer) {
        out_packet->data = (uint8_t*)malloc(out->n_filled_len);
        if (!out_packet->data) {
            svt_av1_enc_release_out_buffer(&out);
            return -2;
        }
        memcpy(out_packet->data, out->p_buffer, out->n_filled_len);
    }

    svt_av1_enc_release_out_buffer(&out);
    return 0;
}

void mp_svt_av1_packet_free(MpSvtAv1Packet* packet) {
    if (!packet) {
        return;
    }
    if (packet->data) {
        free(packet->data);
    }
    packet->data = NULL;
    packet->len = 0;
}

void mp_svt_av1_encoder_destroy(void* encoder) {
    if (!encoder) {
        return;
    }
    MpSvtAv1Encoder* enc = (MpSvtAv1Encoder*)encoder;
    if (enc->handle) {
        svt_av1_enc_deinit(enc->handle);
        svt_av1_enc_deinit_handle(enc->handle);
    }
    free(enc);
}
