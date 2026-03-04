/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifndef EbEncHandle_h
#define EbEncHandle_h

#include "definitions.h"
#include "EbSvtAv1Enc.h"
#include "pic_buffer_desc.h"
#include "sys_resource_manager.h"
#include "sequence_control_set.h"
#include "object.h"

struct _EbThreadContext {
    EbDctor dctor;
    EbPtr   priv;
};

/**************************************
 * Component Private Data
 **************************************/
struct _EbEncHandle {
    EbDctor dctor;
    // Full Results Count
    uint32_t scs_pool_total_count;

    // Config Set Pool & Active Array
    EbSystemResource*             scs_pool_ptr;
    EbSequenceControlSetInstance* scs_instance;

    // Full Results
    EbSystemResource* picture_control_set_pool_ptr;

    EbSystemResource* enc_dec_pool_ptr;

    //ParentControlSet
    EbSystemResource* picture_parent_control_set_pool_ptr;
    EbSystemResource* me_pool_ptr;
    // Picture Buffers
    EbSystemResource* reference_picture_pool_ptr;
    EbSystemResource* tpl_reference_picture_pool_ptr;
    EbSystemResource* pa_reference_picture_pool_ptr;

    // Overlay input picture
    EbSystemResource* overlay_input_picture_pool_ptr;

    // Thread Handles
    EbHandle  resource_coordination_thread_handle;
    EbHandle* picture_analysis_thread_handle_array;
    EbHandle  picture_decision_thread_handle;
    EbHandle* motion_estimation_thread_handle_array;
    EbHandle  initial_rate_control_thread_handle;
    EbHandle* source_based_operations_thread_handle_array;
    EbHandle* tpl_disp_thread_handle_array;
    EbHandle  picture_manager_thread_handle;
    EbHandle  rate_control_thread_handle;
    EbHandle* mode_decision_configuration_thread_handle_array;
    EbHandle* enc_dec_thread_handle_array;
    EbHandle* entropy_coding_thread_handle_array;
    EbHandle* dlf_thread_handle_array;
    EbHandle* cdef_thread_handle_array;
    EbHandle* rest_thread_handle_array;

    EbHandle packetization_thread_handle;

    // Contexts
    EbThreadContext*  resource_coordination_context_ptr;
    EbThreadContext** picture_analysis_context_ptr_array;
    EbThreadContext*  picture_decision_context_ptr;
    EbThreadContext** motion_estimation_context_ptr_array;
    EbThreadContext*  initial_rate_control_context_ptr;
    EbThreadContext** source_based_operations_context_ptr_array;
    EbThreadContext** tpl_disp_context_ptr_array;
    EbThreadContext*  picture_manager_context_ptr;
    EbThreadContext*  rate_control_context_ptr;
    EbThreadContext** mode_decision_configuration_context_ptr_array;
    EbThreadContext** enc_dec_context_ptr_array;
    EbThreadContext** entropy_coding_context_ptr_array;
    EbThreadContext** dlf_context_ptr_array;
    EbThreadContext** cdef_context_ptr_array;
    EbThreadContext** rest_context_ptr_array;
    EbThreadContext*  packetization_context_ptr;

    // System Resource Managers
    EbSystemResource* input_buffer_resource_ptr;
    EbSystemResource* input_y8b_buffer_resource_ptr;
    EbSystemResource* input_cmd_resource_ptr;
    EbSystemResource* output_stream_buffer_resource_ptr;
    EbSystemResource* output_recon_buffer_resource_ptr;
    EbSystemResource* resource_coordination_results_resource_ptr;
    EbSystemResource* picture_analysis_results_resource_ptr;
    EbSystemResource* picture_decision_results_resource_ptr;
    EbSystemResource* motion_estimation_results_resource_ptr;
    EbSystemResource* initial_rate_control_results_resource_ptr;
    EbSystemResource* picture_demux_results_resource_ptr;
    EbSystemResource* tpl_disp_res_srm;
    EbSystemResource* rate_control_tasks_resource_ptr;
    EbSystemResource* rate_control_results_resource_ptr;
    EbSystemResource* enc_dec_tasks_resource_ptr;
    EbSystemResource* enc_dec_results_resource_ptr;
    EbSystemResource* entropy_coding_results_resource_ptr;
    EbSystemResource* dlf_results_resource_ptr;
    EbSystemResource* cdef_results_resource_ptr;
    EbSystemResource* rest_results_resource_ptr;

    // Callbacks
    EbCallback* app_callback_ptr;

    EbFifo* input_buffer_producer_fifo_ptr;
    EbFifo* input_cmd_producer_fifo_ptr;
    EbFifo* input_y8b_buffer_producer_fifo_ptr;
    EbFifo* output_stream_buffer_consumer_fifo_ptr;
    EbFifo* output_recon_buffer_consumer_fifo_ptr;

    bool eos_received; // used to signal we received the EOS from the app
    bool eos_sent; // used to signal we sent the EOS to the app
    bool frame_received; // used to signal we received any frame from the app
    bool is_prev_valid; // whether the previous input is valid or not
};

void set_segments_numbers(SequenceControlSet* scs);
#endif // EbEncHandle_h
