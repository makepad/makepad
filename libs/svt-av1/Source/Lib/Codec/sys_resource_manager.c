/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2019, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <stdlib.h>

#include "sys_resource_manager.h"
#include "definitions.h"
#include "svt_threads.h"
#if SRM_REPORT
#include "svt_log.h"
#endif
static void svt_fifo_dctor(EbPtr p) {
    EbFifo* obj = (EbFifo*)p;
    EB_DESTROY_SEMAPHORE(obj->counting_semaphore);
    EB_DESTROY_MUTEX(obj->lockout_mutex);
}

/**************************************
 * svt_fifo_ctor
 **************************************/
static EbErrorType svt_fifo_ctor(EbFifo* fifoPtr, uint32_t initial_count, uint32_t max_count,
                                 EbObjectWrapper* firstWrapperPtr, EbObjectWrapper* lastWrapperPtr,
                                 EbMuxingQueue* queue_ptr) {
    fifoPtr->dctor = svt_fifo_dctor;
    // Create Counting Semaphore
    EB_CREATE_SEMAPHORE(fifoPtr->counting_semaphore, initial_count, max_count);

    // Create Buffer Pool Mutex
    EB_CREATE_MUTEX(fifoPtr->lockout_mutex);

    // Initialize Fifo First & Last ptrs
    fifoPtr->first_ptr = firstWrapperPtr;
    fifoPtr->last_ptr  = lastWrapperPtr;

    // Copy the Muxing Queue ptr this Fifo belongs to
    fifoPtr->queue_ptr = queue_ptr;

    return EB_ErrorNone;
}

/**************************************
 * svt_fifo_push_back
 **************************************/
static EbErrorType svt_fifo_push_back(EbFifo* fifoPtr, EbObjectWrapper* wrapper_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    // If FIFO is empty
    if (fifoPtr->first_ptr == NULL) {
        fifoPtr->first_ptr = wrapper_ptr;
        fifoPtr->last_ptr  = wrapper_ptr;
    } else {
        fifoPtr->last_ptr->next_ptr = wrapper_ptr;
        fifoPtr->last_ptr           = wrapper_ptr;
    }

    fifoPtr->last_ptr->next_ptr = NULL;

    return return_error;
}

/**************************************
 * svt_fifo_pop_front
 **************************************/
static EbErrorType svt_fifo_pop_front(EbFifo* fifoPtr, EbObjectWrapper** wrapper_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    // Set wrapper_ptr to head of BufferPool
    *wrapper_ptr = fifoPtr->first_ptr;

    // Update tail of BufferPool if the BufferPool is now empty
    fifoPtr->last_ptr = (fifoPtr->first_ptr == fifoPtr->last_ptr) ? NULL : fifoPtr->last_ptr;

    // Update head of BufferPool
    fifoPtr->first_ptr = fifoPtr->first_ptr->next_ptr;

    return return_error;
}

static EbErrorType svt_fifo_shutdown(EbFifo* fifo_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    // Acquire lockout Mutex
    svt_block_on_mutex(fifo_ptr->lockout_mutex);
    fifo_ptr->quit_signal = true;
    // Release Mutex
    svt_release_mutex(fifo_ptr->lockout_mutex);
    //Wake up the waiting process if any
    svt_post_semaphore(fifo_ptr->counting_semaphore);

    return return_error;
}

static void svt_circular_buffer_dctor(EbPtr p) {
    EbCircularBuffer* obj = (EbCircularBuffer*)p;
    EB_FREE(obj->array_ptr);
}

/**************************************
 * svt_circular_buffer_ctor
 **************************************/
static EbErrorType svt_circular_buffer_ctor(EbCircularBuffer* bufferPtr, uint32_t buffer_total_count) {
    bufferPtr->dctor = svt_circular_buffer_dctor;

    bufferPtr->buffer_total_count = buffer_total_count;

    EB_CALLOC(bufferPtr->array_ptr, bufferPtr->buffer_total_count, sizeof(EbPtr));

    return EB_ErrorNone;
}

/**************************************
 * svt_circular_buffer_empty_check
 **************************************/
static bool svt_circular_buffer_empty_check(EbCircularBuffer* bufferPtr) {
    return ((bufferPtr->head_index == bufferPtr->tail_index) && (bufferPtr->array_ptr[bufferPtr->head_index] == NULL))
        ? true
        : false;
}

/**************************************
 * svt_circular_buffer_pop_front
 **************************************/
static EbErrorType svt_circular_buffer_pop_front(EbCircularBuffer* bufferPtr, EbPtr* object_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    // Copy the head of the buffer into the object_ptr
    *object_ptr                                 = bufferPtr->array_ptr[bufferPtr->head_index];
    bufferPtr->array_ptr[bufferPtr->head_index] = NULL;

    // Increment the head & check for rollover
    bufferPtr->head_index = (bufferPtr->head_index == bufferPtr->buffer_total_count - 1) ? 0
                                                                                         : bufferPtr->head_index + 1;

    // Decrement the Current Count
    --bufferPtr->current_count;

    return return_error;
}

/**************************************
 * svt_circular_buffer_push_back
 **************************************/
static EbErrorType svt_circular_buffer_push_back(EbCircularBuffer* bufferPtr, EbPtr object_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    // Copy the pointer into the array
    bufferPtr->array_ptr[bufferPtr->tail_index] = object_ptr;

    // Increment the tail & check for rollover
    bufferPtr->tail_index = (bufferPtr->tail_index == bufferPtr->buffer_total_count - 1) ? 0
                                                                                         : bufferPtr->tail_index + 1;

    // Increment the Current Count
    ++bufferPtr->current_count;

    return return_error;
}

/**************************************
 * svt_circular_buffer_push_front
 **************************************/
static EbErrorType svt_circular_buffer_push_front(EbCircularBuffer* bufferPtr, EbPtr object_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    // Decrement the head_index
    bufferPtr->head_index = (bufferPtr->head_index == 0) ? bufferPtr->buffer_total_count - 1
                                                         : bufferPtr->head_index - 1;

    // Copy the pointer into the array
    bufferPtr->array_ptr[bufferPtr->head_index] = object_ptr;

    // Increment the Current Count
    ++bufferPtr->current_count;

    return return_error;
}

void svt_muxing_queue_dctor(EbPtr p) {
    EbMuxingQueue* obj = (EbMuxingQueue*)p;
    EB_DELETE_PTR_ARRAY(obj->process_fifo_ptr_array, obj->process_total_count);
    EB_DELETE(obj->object_queue);
    EB_DELETE(obj->process_queue);
    EB_DESTROY_MUTEX(obj->lockout_mutex);
}

/**************************************
 * svt_muxing_queue_ctor
 **************************************/
static EbErrorType svt_muxing_queue_ctor(EbMuxingQueue* queue_ptr, uint32_t object_total_count,
                                         uint32_t process_total_count) {
    uint32_t    process_index;
    EbErrorType return_error = EB_ErrorNone;

    queue_ptr->dctor               = svt_muxing_queue_dctor;
    queue_ptr->process_total_count = process_total_count;

    // Lockout Mutex
    EB_CREATE_MUTEX(queue_ptr->lockout_mutex);

    // Construct Object Circular Buffer
    EB_NEW(queue_ptr->object_queue, svt_circular_buffer_ctor, object_total_count);
    // Construct Process Circular Buffer
    EB_NEW(queue_ptr->process_queue, svt_circular_buffer_ctor, queue_ptr->process_total_count);
    // Construct the Process Fifos
    EB_ALLOC_PTR_ARRAY(queue_ptr->process_fifo_ptr_array, queue_ptr->process_total_count);

    for (process_index = 0; process_index < queue_ptr->process_total_count; ++process_index) {
        EB_NEW(queue_ptr->process_fifo_ptr_array[process_index],
               svt_fifo_ctor,
               0,
               object_total_count,
               NULL,
               NULL,
               queue_ptr);
    }

    return return_error;
}

/**************************************
 * svt_muxing_queue_assignation
 **************************************/
static EbErrorType svt_muxing_queue_assignation(EbMuxingQueue* queue_ptr) {
    EbErrorType      return_error = EB_ErrorNone;
    EbFifo*          process_fifo_ptr;
    EbObjectWrapper* wrapper_ptr;

    // while loop
    while ((svt_circular_buffer_empty_check(queue_ptr->object_queue) == false) &&
           (svt_circular_buffer_empty_check(queue_ptr->process_queue) == false)) {
        // Get the next process
        svt_circular_buffer_pop_front(queue_ptr->process_queue, (void**)&process_fifo_ptr);

        // Get the next object
        svt_circular_buffer_pop_front(queue_ptr->object_queue, (void**)&wrapper_ptr);

        // Block on the Process Fifo's Mutex
        svt_block_on_mutex(process_fifo_ptr->lockout_mutex);

        // Put the object on the fifo
        svt_fifo_push_back(process_fifo_ptr, wrapper_ptr);

        // Release the Process Fifo's Mutex
        svt_release_mutex(process_fifo_ptr->lockout_mutex);

        // Post the semaphore
        svt_post_semaphore(process_fifo_ptr->counting_semaphore);
    }

    return return_error;
}

/**************************************
 * svt_muxing_queue_object_push_back
 **************************************/
static EbErrorType svt_muxing_queue_object_push_back(EbMuxingQueue* queue_ptr, EbObjectWrapper* object_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    svt_circular_buffer_push_back(queue_ptr->object_queue, object_ptr);

    svt_muxing_queue_assignation(queue_ptr);

    return return_error;
}

/**************************************
* svt_muxing_queue_object_push_front
**************************************/
static EbErrorType svt_muxing_queue_object_push_front(EbMuxingQueue* queue_ptr, EbObjectWrapper* object_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    svt_circular_buffer_push_front(queue_ptr->object_queue, object_ptr);

    svt_muxing_queue_assignation(queue_ptr);

    return return_error;
}

static EbFifo* svt_muxing_queue_get_fifo(EbMuxingQueue* queue_ptr, uint32_t index) {
    assert(queue_ptr->process_fifo_ptr_array && (queue_ptr->process_total_count > index));
    return queue_ptr->process_fifo_ptr_array[index];
}

/*********************************************************************
 * svt_object_release_enable
 *   Enables the release_enable member of EbObjectWrapper.  Used by
 *   certain objects (e.g. SequenceControlSet) to control whether
 *   EbObjectWrappers are allowed to be released or not.
 *
 *   resource_ptr
 *      pointer to the SystemResource that manages the EbObjectWrapper.
 *      The emptyFifo's lockout_mutex is used to write protect the
 *      modification of the EbObjectWrapper.
 *
 *   wrapper_ptr
 *      pointer to the EbObjectWrapper to be modified.
 *********************************************************************/
EbErrorType svt_object_release_enable(EbObjectWrapper* wrapper_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    svt_block_on_mutex(wrapper_ptr->system_resource_ptr->empty_queue->lockout_mutex);

    wrapper_ptr->release_enable = true;

    svt_release_mutex(wrapper_ptr->system_resource_ptr->empty_queue->lockout_mutex);

    return return_error;
}

/*********************************************************************
 * svt_object_release_disable
 *   Disables the release_enable member of EbObjectWrapper.  Used by
 *   certain objects (e.g. SequenceControlSet) to control whether
 *   EbObjectWrappers are allowed to be released or not.
 *
 *   resource_ptr
 *      pointer to the SystemResource that manages the EbObjectWrapper.
 *      The emptyFifo's lockout_mutex is used to write protect the
 *      modification of the EbObjectWrapper.
 *
 *   wrapper_ptr
 *      pointer to the EbObjectWrapper to be modified.
 *********************************************************************/
EbErrorType svt_object_release_disable(EbObjectWrapper* wrapper_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    svt_block_on_mutex(wrapper_ptr->system_resource_ptr->empty_queue->lockout_mutex);

    wrapper_ptr->release_enable = false;

    svt_release_mutex(wrapper_ptr->system_resource_ptr->empty_queue->lockout_mutex);

    return return_error;
}

/*********************************************************************
 * svt_object_inc_live_count
 *   Increments the live_count member of EbObjectWrapper.  Used by
 *   certain objects (e.g. SequenceControlSet) to count the number of active
 *   pointers of a EbObjectWrapper in pipeline at any point in time.
 *
 *   resource_ptr
 *      pointer to the SystemResource that manages the EbObjectWrapper.
 *      The emptyFifo's lockout_mutex is used to write protect the
 *      modification of the EbObjectWrapper.
 *
 *   wrapper_ptr
 *      pointer to the EbObjectWrapper to be modified.
 *********************************************************************/
EbErrorType svt_object_inc_live_count(EbObjectWrapper* wrapper_ptr, uint32_t increment_number) {
    EbErrorType return_error = EB_ErrorNone;

    svt_block_on_mutex(wrapper_ptr->system_resource_ptr->empty_queue->lockout_mutex);

    svt_aom_assert_err(wrapper_ptr->live_count != EB_ObjectWrapperReleasedValue,
                       "live_count should not be EB_ObjectWrapperReleasedValue when inc");

    wrapper_ptr->live_count += increment_number;

    svt_release_mutex(wrapper_ptr->system_resource_ptr->empty_queue->lockout_mutex);

    return return_error;
}

//ugly hack
typedef struct DctorAble {
    EbDctor dctor;
} DctorAble;

void svt_object_wrapper_dctor(EbPtr p) {
    EbObjectWrapper* wrapper = (EbObjectWrapper*)p;
    if (wrapper->object_destroyer) {
        //customized destroyer
        if (wrapper->object_ptr) {
            wrapper->object_destroyer(wrapper->object_ptr);
        }
    } else {
        //hack....
        DctorAble* obj = (DctorAble*)wrapper->object_ptr;
        EB_DELETE(obj);
    }
}

static EbErrorType svt_object_wrapper_ctor(EbObjectWrapper* wrapper, EbSystemResource* resource,
                                           EbCreator object_creator, EbPtr object_init_data_ptr,
                                           EbDctor object_destroyer) {
    EbErrorType ret;

    wrapper->dctor               = svt_object_wrapper_dctor;
    wrapper->release_enable      = true;
    wrapper->system_resource_ptr = resource;
    wrapper->object_destroyer    = object_destroyer;
    ret                          = object_creator(&wrapper->object_ptr, object_init_data_ptr);
    if (ret != EB_ErrorNone) {
        return ret;
    }
    return EB_ErrorNone;
}

static void svt_system_resource_dctor(EbPtr p) {
    EbSystemResource* obj = (EbSystemResource*)p;
    EB_DELETE(obj->full_queue);
    EB_DELETE(obj->empty_queue);
    EB_DELETE_PTR_ARRAY(obj->wrapper_ptr_pool, obj->object_total_count);
}

/*********************************************************************
 * svt_system_resource_ctor
 *   Constructor for EbSystemResource.  Fully constructs all members
 *   of EbSystemResource including the object with the passed
 *   object_ctor function.
 *
 *   resource_ptr
 *     pointer that will contain the SystemResource to be constructed.
 *
 *   object_total_count
 *     Number of objects to be managed by the SystemResource.
 *
 *   object_ctor
 *     Function pointer to the constructor of the object managed by
 *     SystemResource referenced by resource_ptr. No object level
 *     construction is performed if object_ctor is NULL.
 *
 *   object_init_data_ptr

 *     pointer to data block to be used during the construction of
 *     the object. object_init_data_ptr is passed to object_ctor when
 *     object_ctor is called.
 *   object_destroyer
 *     object destroyer, will call dctor if this is null
 *********************************************************************/
EbErrorType svt_system_resource_ctor(EbSystemResource* resource_ptr, uint32_t object_total_count,
                                     uint32_t producer_process_total_count, uint32_t consumer_process_total_count,
                                     EbCreator object_creator, EbPtr object_init_data_ptr, EbDctor object_destroyer) {
    uint32_t    wrapper_index;
    EbErrorType return_error = EB_ErrorNone;
    resource_ptr->dctor      = svt_system_resource_dctor;

    resource_ptr->object_total_count = object_total_count;

    // Allocate array for wrapper pointers
    EB_ALLOC_PTR_ARRAY(resource_ptr->wrapper_ptr_pool, resource_ptr->object_total_count);

    // Initialize each wrapper
    for (wrapper_index = 0; wrapper_index < resource_ptr->object_total_count; ++wrapper_index) {
        EB_NEW(resource_ptr->wrapper_ptr_pool[wrapper_index],
               svt_object_wrapper_ctor,
               resource_ptr,
               object_creator,
               object_init_data_ptr,
               object_destroyer);

#if SRM_REPORT
        resource_ptr->wrapper_ptr_pool[wrapper_index]->pic_number = 99999999;
#endif
    }

    // Initialize the Empty Queue
    EB_NEW(resource_ptr->empty_queue,
           svt_muxing_queue_ctor,
           resource_ptr->object_total_count,
           producer_process_total_count);
    // Fill the Empty Fifo with every ObjectWrapper
    for (wrapper_index = 0; wrapper_index < resource_ptr->object_total_count; ++wrapper_index) {
        svt_muxing_queue_object_push_back(resource_ptr->empty_queue, resource_ptr->wrapper_ptr_pool[wrapper_index]);
    }
#if SRM_REPORT
    //at init time, the SRM is full
    resource_ptr->empty_queue->curr_count = resource_ptr->object_total_count;
    resource_ptr->empty_queue->log        = 0;
#endif
    // Initialize the Full Queue
    if (consumer_process_total_count) {
        EB_NEW(resource_ptr->full_queue,
               svt_muxing_queue_ctor,
               resource_ptr->object_total_count,
               consumer_process_total_count);
    } else {
        resource_ptr->full_queue = NULL;
    }

    return return_error;
}

EbFifo* svt_system_resource_get_producer_fifo(const EbSystemResource* resource_ptr, uint32_t index) {
    return svt_muxing_queue_get_fifo(resource_ptr->empty_queue, index);
}

EbFifo* svt_system_resource_get_consumer_fifo(const EbSystemResource* resource_ptr, uint32_t index) {
    return svt_muxing_queue_get_fifo(resource_ptr->full_queue, index);
}

EbErrorType svt_shutdown_process(const EbSystemResource* resource_ptr) {
    //not fully constructed
    if (!resource_ptr || !resource_ptr->full_queue) {
        return EB_ErrorNone;
    }

    //notify all consumers we are shutting down
    for (unsigned int i = 0; i < resource_ptr->full_queue->process_total_count; i++) {
        EbFifo* fifo_ptr = svt_system_resource_get_consumer_fifo(resource_ptr, i);
        svt_fifo_shutdown(fifo_ptr);
    }
    return EB_ErrorNone;
}

/*********************************************************************
 * EbSystemResourceReleaseProcess
 *********************************************************************/
static EbErrorType svt_release_process(EbFifo* process_fifo_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    svt_block_on_mutex(process_fifo_ptr->queue_ptr->lockout_mutex);

    svt_circular_buffer_push_front(process_fifo_ptr->queue_ptr->process_queue, process_fifo_ptr);

    svt_muxing_queue_assignation(process_fifo_ptr->queue_ptr);

    svt_release_mutex(process_fifo_ptr->queue_ptr->lockout_mutex);

    return return_error;
}

/*********************************************************************
 * EbSystemResourcePostObject
 *   Queues a full EbObjectWrapper to the SystemResource. This
 *   function posts the SystemResource fullFifo counting_semaphore.
 *   This function is write protected by the SystemResource fullFifo
 *   lockout_mutex.
 *
 *   resource_ptr
 *      pointer to the SystemResource that the EbObjectWrapper is
 *      posted to.
 *
 *   wrapper_ptr
 *      pointer to EbObjectWrapper to be posted.
 *********************************************************************/
EbErrorType svt_post_full_object(EbObjectWrapper* object_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    svt_block_on_mutex(object_ptr->system_resource_ptr->full_queue->lockout_mutex);

    svt_muxing_queue_object_push_back(object_ptr->system_resource_ptr->full_queue, object_ptr);

    svt_release_mutex(object_ptr->system_resource_ptr->full_queue->lockout_mutex);

    return return_error;
}

/*********************************************************************
 * EbSystemResourceReleaseObject
 *   Queues an empty EbObjectWrapper to the SystemResource. This
 *   function posts the SystemResource emptyFifo counting_semaphore.
 *   This function is write protected by the SystemResource emptyFifo
 *   lockout_mutex.
 *
 *   object_ptr
 *      pointer to EbObjectWrapper to be released.
 *********************************************************************/
EbErrorType svt_release_object(EbObjectWrapper* object_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    svt_block_on_mutex(object_ptr->system_resource_ptr->empty_queue->lockout_mutex);

    svt_aom_assert_err(object_ptr->live_count != EB_ObjectWrapperReleasedValue,
                       "live_count should not be EB_ObjectWrapperReleasedValue when release");

    // Decrement live_count
    object_ptr->live_count = (object_ptr->live_count == 0) ? object_ptr->live_count : object_ptr->live_count - 1;

    if ((object_ptr->release_enable == true) && (object_ptr->live_count == 0)) {
        // Set live_count to EB_ObjectWrapperReleasedValue
        object_ptr->live_count = EB_ObjectWrapperReleasedValue;

        svt_muxing_queue_object_push_front(object_ptr->system_resource_ptr->empty_queue, object_ptr);
#if SRM_REPORT
        object_ptr->pic_number = 99999999;
        //increment the fullness
        object_ptr->system_resource_ptr->empty_queue->curr_count++;
        if (object_ptr->system_resource_ptr->empty_queue->log) {
            SVT_LOG("SRM fullness+: %i/%i\n",
                    object_ptr->system_resource_ptr->empty_queue->curr_count,
                    object_ptr->system_resource_ptr->object_total_count);
        }
#endif
    }

    svt_release_mutex(object_ptr->system_resource_ptr->empty_queue->lockout_mutex);

    return return_error;
}

EbErrorType svt_release_dual_object(EbObjectWrapper* object_ptr, EbObjectWrapper* sec_object_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    svt_block_on_mutex(object_ptr->system_resource_ptr->empty_queue->lockout_mutex);

    // Decrement live_count
    object_ptr->live_count = (object_ptr->live_count == 0) ? object_ptr->live_count : object_ptr->live_count - 1;

    if ((object_ptr->release_enable == true) && (object_ptr->live_count == 0)) {
        //release the second object
        svt_release_object(sec_object_ptr);

        // Set live_count to EB_ObjectWrapperReleasedValue
        object_ptr->live_count = EB_ObjectWrapperReleasedValue;

        svt_muxing_queue_object_push_front(object_ptr->system_resource_ptr->empty_queue, object_ptr);

#if SRM_REPORT

        if (object_ptr->system_resource_ptr->empty_queue->log) {
            SVT_LOG("SRM RELEASE: %lld\n", object_ptr->pic_number);
        }

        object_ptr->pic_number = 99999999;
        //increment the fullness
        object_ptr->system_resource_ptr->empty_queue->curr_count++;
        //  if (object_ptr->system_resource_ptr->empty_queue->log)
        //      SVT_LOG("SRM fullness+: %i/%i\n", object_ptr->system_resource_ptr->empty_queue->curr_count, object_ptr->system_resource_ptr->object_total_count);
#endif
    }

    svt_release_mutex(object_ptr->system_resource_ptr->empty_queue->lockout_mutex);

    return return_error;
}
#if SRM_REPORT
/*
  dump pictures occuping the SRM
*/
EbErrorType dump_srm_content(EbSystemResource* resource_ptr, uint8_t log) {
    EbErrorType return_error = EB_ErrorNone;
    if (log) {
        SVT_LOG("SRM content:\n\n");
        for (uint32_t wrapper_index = 0; wrapper_index < resource_ptr->object_total_count; ++wrapper_index) {
            SVT_LOG("%lld ", resource_ptr->wrapper_ptr_pool[wrapper_index]->pic_number);
        }
    }
    return return_error;
}
#endif
/*********************************************************************
 * EbSystemResourceGetEmptyObject
 *   Dequeues an empty EbObjectWrapper from the SystemResource.  This
 *   function blocks on the SystemResource emptyFifo counting_semaphore.
 *   This function is write protected by the SystemResource emptyFifo
 *   lockout_mutex.
 *
 *   resource_ptr
 *      pointer to the SystemResource that provides the empty
 *      EbObjectWrapper.
 *
 *   wrapper_dbl_ptr
 *      Double pointer used to pass the pointer to the empty
 *      EbObjectWrapper pointer.
 *********************************************************************/
EbErrorType svt_get_empty_object(EbFifo* empty_fifo_ptr, EbObjectWrapper** wrapper_dbl_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    // Queue the Fifo requesting the empty fifo
    svt_release_process(empty_fifo_ptr);

    // Block on the counting Semaphore until an empty buffer is available
    svt_block_on_semaphore(empty_fifo_ptr->counting_semaphore);

    // Acquire lockout Mutex
    svt_block_on_mutex(empty_fifo_ptr->lockout_mutex);

    // Get the empty object
    svt_fifo_pop_front(empty_fifo_ptr, wrapper_dbl_ptr);

#if SRM_REPORT
    //decrement the fullness
    empty_fifo_ptr->queue_ptr->curr_count--;
    if (empty_fifo_ptr->queue_ptr->log) {
        printf("SRM fullness-: %i/%i\n",
               empty_fifo_ptr->queue_ptr->curr_count,
               (*wrapper_dbl_ptr)->system_resource_ptr->object_total_count);
    }
#endif

    svt_aom_assert_err(
        (*wrapper_dbl_ptr)->live_count == 0 || (*wrapper_dbl_ptr)->live_count == EB_ObjectWrapperReleasedValue,
        "live_count should be 0 or EB_ObjectWrapperReleasedValue when get");

    // Reset the wrapper's live_count
    (*wrapper_dbl_ptr)->live_count = 0;

    // Object release enable
    (*wrapper_dbl_ptr)->release_enable = true;

    // Release Mutex
    svt_release_mutex(empty_fifo_ptr->lockout_mutex);

    return return_error;
}

/*********************************************************************
 * EbSystemResourceGetFullObject
 *   Dequeues an full EbObjectWrapper from the SystemResource. This
 *   function blocks on the SystemResource fullFifo counting_semaphore.
 *   This function is write protected by the SystemResource fullFifo
 *   lockout_mutex.
 *
 *   resource_ptr
 *      pointer to the SystemResource that provides the full
 *      EbObjectWrapper.
 *
 *   wrapper_dbl_ptr
 *      Double pointer used to pass the pointer to the full
 *      EbObjectWrapper pointer.
 *********************************************************************/
EbErrorType svt_get_full_object(EbFifo* full_fifo_ptr, EbObjectWrapper** wrapper_dbl_ptr) {
    EbErrorType return_error = EB_ErrorNone;

    // Queue the Fifo requesting the full fifo
    svt_release_process(full_fifo_ptr);

    // Block on the counting Semaphore until an empty buffer is available
    svt_block_on_semaphore(full_fifo_ptr->counting_semaphore);

    // Acquire lockout Mutex
    svt_block_on_mutex(full_fifo_ptr->lockout_mutex);

    if (!full_fifo_ptr->quit_signal) {
        svt_fifo_pop_front(full_fifo_ptr, wrapper_dbl_ptr);
    } else {
        *wrapper_dbl_ptr = NULL;
        return_error     = EB_NoErrorFifoShutdown;
    }

    // Release Mutex
    svt_release_mutex(full_fifo_ptr->lockout_mutex);

    return return_error;
}

/**************************************
* svt_fifo_pop_front
**************************************/
static bool svt_fifo_peak_front(EbFifo* fifoPtr) {
    // Set wrapper_ptr to head of BufferPool
    if (fifoPtr->first_ptr == NULL) {
        return true;
    } else {
        return false;
    }
}

EbErrorType svt_get_full_object_non_blocking(EbFifo* full_fifo_ptr, EbObjectWrapper** wrapper_dbl_ptr) {
    EbErrorType return_error = EB_ErrorNone;
    bool        fifo_empty;
    // Queue the Fifo requesting the full fifo
    svt_release_process(full_fifo_ptr);

    // Acquire lockout Mutex
    svt_block_on_mutex(full_fifo_ptr->lockout_mutex);

    //if the fifo is shutting down, we will not give any buffer to caller
    if (!full_fifo_ptr->quit_signal) {
        fifo_empty = svt_fifo_peak_front(full_fifo_ptr);
    } else {
        fifo_empty = true;
    }

    // Release Mutex
    svt_release_mutex(full_fifo_ptr->lockout_mutex);

    if (fifo_empty == false) {
        svt_get_full_object(full_fifo_ptr, wrapper_dbl_ptr);
    } else {
        *wrapper_dbl_ptr = NULL;
    }

    return return_error;
}
