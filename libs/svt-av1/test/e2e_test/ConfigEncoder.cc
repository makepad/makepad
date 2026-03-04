/*
 * Copyright(c) 2019 Intel Corporation
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

#include <string>

extern "C" {
#include "app_config.c"
}

void set_enc_config(void *config_ptr, const char *name, const char *value) {
    EbConfig *app_cfg = (EbConfig *)config_ptr;
    set_config_value(app_cfg, name, value);
}

bool set_default_config(EbSvtAv1EncConfiguration *config) {
    EbComponentType *handle;
    if (svt_av1_enc_init_handle(&handle, config) != EB_ErrorNone) {
        return false;
    }
    svt_av1_enc_deinit_handle(handle);
    return true;
}

void release_enc_config(void *config_ptr) {
    if (config_ptr) {
        EbConfig *app_cfg = (EbConfig *)config_ptr;
        svt_config_dtor(app_cfg);
    }
}

void *create_enc_config() {
    EbConfig *app_cfg = svt_config_ctor();
    assert(app_cfg != NULL);
    if (!set_default_config(&app_cfg->config)) {
        release_enc_config(app_cfg);
        app_cfg = NULL;
    }
    assert(app_cfg != NULL);
    return app_cfg;
}

int copy_enc_param(EbSvtAv1EncConfiguration *dst_enc_config, void *config_ptr) {
    EbConfig *app_cfg = (EbConfig *)config_ptr;
    memcpy(dst_enc_config, &app_cfg->config, sizeof(EbSvtAv1EncConfiguration));
    return 0;
}

extern ConfigEntry config_entry[];
std::string get_enc_token(const char *name) {
    std::string str;
    int index = 0;
    while (config_entry[index].name != NULL) {
        if (strcmp(name, config_entry[index].name) == 0) {
            str = config_entry[index].token;
            break;
        }
        index++;
    }
    return str;
}
