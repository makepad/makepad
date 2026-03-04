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

#ifndef EbSystemResource_h
#define EbSystemResource_h

#include "object.h"
#ifdef __cplusplus
extern "C" {
#endif
/*********************************
     * Defines
     *********************************/
#define EB_ObjectWrapperReleasedValue ~0u

/*********************************************************************
      * Object Wrapper
      *   Provides state information for each type of object in the
      *   encoder system (i.e. SequenceControlSet, PictureBufferDesc,
      *   ProcessResults, and GracefulDegradation)
      *********************************************************************/
typedef struct EbObjectWrapper {
    EbDctor dctor;

    EbDctor object_destroyer;
    // object_ptr - pointer to the object being managed.
    void* object_ptr;

    // live_count - a count of the number of pictures actively being
    //   encoded in the pipeline at any given time.  Modification
    //   of this value by any process must be protected by a mutex.
    uint32_t live_count;

    // release_enable - a flag that enables the release of
    //   EbObjectWrapper for reuse in the encoding of subsequent
    //   pictures in the encoder pipeline.
    bool release_enable;

    // system_resource_ptr - a pointer to the SystemResourceManager
    //   that the object belongs to.
    struct EbSystemResource* system_resource_ptr;

    // next_ptr - a pointer to a different EbObjectWrapper.  Used
    //   only in the implemenation of a single-linked Fifo.
    struct EbObjectWrapper* next_ptr;
#if SRM_REPORT
    uint64_t pic_number;
#endif
} EbObjectWrapper;

/*********************************************************************
     * Fifo
     *   Defines a static (i.e. no dynamic memory allocation) single
     *   linked-list, constant time fifo implementation. The fifo uses
     *   the EbObjectWrapper member next_ptr to create the linked-list.
     *   The Fifo also contains a counting_semaphore for OS thread-blocking
     *   and dynamic EbObjectWrapper counting.
     *********************************************************************/
typedef struct EbFifo {
    EbDctor dctor;
    // counting_semaphore - used for OS thread-blocking & dynamically
    //   counting the number of EbObjectWrappers currently in the
    //   EbFifo.
    EbHandle counting_semaphore;

    // lockout_mutex - used to prevent more than one thread from
    //   modifying EbFifo simultaneously.
    EbHandle lockout_mutex;

    // first_ptr - pointer the the head of the Fifo
    EbObjectWrapper* first_ptr;

    // last_ptr - pointer to the tail of the Fifo
    EbObjectWrapper* last_ptr;

    // quit_signal - a flag that main thread sets to break out from kernels
    bool quit_signal;

    // queue_ptr - pointer to MuxingQueue that the EbFifo is
    //   associated with.
    struct EbMuxingQueue* queue_ptr;
} EbFifo;

/*********************************************************************
     * CircularBuffer
     *********************************************************************/
typedef struct EbCircularBuffer {
    EbDctor  dctor;
    EbPtr*   array_ptr;
    uint32_t head_index;
    uint32_t tail_index;
    uint32_t buffer_total_count;
    uint32_t current_count;
} EbCircularBuffer;

/*********************************************************************
     * MuxingQueue
     *********************************************************************/
typedef struct EbMuxingQueue {
    EbDctor           dctor;
    EbHandle          lockout_mutex;
    EbCircularBuffer* object_queue;
    EbCircularBuffer* process_queue;
    uint32_t          process_total_count;
    EbFifo**          process_fifo_ptr_array;
#if SRM_REPORT
    uint32_t curr_count; //run time fullness
    uint8_t  log; //if set monitor out the queue size
#endif
} EbMuxingQueue;

/*********************************************************************
     * SystemResource
     *   Defines a complete solution for managing objects in the encoder
     *   system (i.e. SequenceControlSet, PictureBufferDesc, ProcessResults, and
     *   GracefulDegradation).  The object_total_count and wrapper_ptr_pool are
     *   only used to construct and destruct the SystemResource.  The
     *   fullFifo provides downstream pipeline data flow control.  The
     *   emptyFifo provides upstream pipeline backpressure flow control.
     *********************************************************************/
typedef struct EbSystemResource {
    EbDctor dctor;
    // object_total_count - A count of the number of objects contained in the
    //   System Resoruce.
    uint32_t object_total_count;

    // wrapper_ptr_pool - An array of pointers to the EbObjectWrappers used
    //   to construct and destruct the SystemResource.
    EbObjectWrapper** wrapper_ptr_pool;

    // The empty FIFO contains a queue of empty buffers
    EbMuxingQueue* empty_queue;

    // The full FIFO contains a queue of completed buffers
    EbMuxingQueue* full_queue;
} EbSystemResource;

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
EbErrorType svt_object_release_enable(EbObjectWrapper* wrapper_ptr);

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
EbErrorType svt_object_release_disable(EbObjectWrapper* wrapper_ptr);

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
     *
     *   increment_number
     *      The number to increment the live count by.
     *********************************************************************/
EbErrorType svt_object_inc_live_count(EbObjectWrapper* wrapper_ptr, uint32_t increment_number);

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
     *********************************************************************/
EbErrorType svt_system_resource_ctor(EbSystemResource* resource_ptr, uint32_t object_total_count,
                                     uint32_t producer_process_total_count, uint32_t consumer_process_total_count,
                                     EbCreator object_ctor, EbPtr object_init_data_ptr, EbDctor object_destroyer);

/*********************************************************************
     * svt_system_resource_get_producer_fifo
     *   get producer fifo
     *
     *   resource_ptr
     *     pointer to SystemResource
     *
     *   index
     *     index to the producer fifo
     */
EbFifo* svt_system_resource_get_producer_fifo(const EbSystemResource* resource_ptr, uint32_t index);

/*********************************************************************
     * svt_system_resource_get_consumer_fifo
     *   get producer fifo
     *
     *   resource_ptr
     *     pointer to SystemResource
     *
     *   index
     *     index to the consumer fifo
     */
EbFifo* svt_system_resource_get_consumer_fifo(const EbSystemResource* resource_ptr, uint32_t index);

/*********************************************************************
     * EbSystemResourceGetEmptyObject
     *   Dequeues an empty EbObjectWrapper from the SystemResource.  The
     *   new EbObjectWrapper will be populated with the contents of the
     *   wrapperCopyPtr if wrapperCopyPtr is not NULL. This function blocks
     *   on the SystemResource emptyFifo counting_semaphore. This function
     *   is write protected by the SystemResource emptyFifo lockout_mutex.
     *
     *   resource_ptr
     *      pointer to the SystemResource that provides the empty
     *      EbObjectWrapper.
     *
     *   wrapper_dbl_ptr
     *      Double pointer used to pass the pointer to the empty
     *      EbObjectWrapper pointer.
     *********************************************************************/
EbErrorType svt_get_empty_object(EbFifo* empty_fifo_ptr, EbObjectWrapper** wrapper_dbl_ptr);
#if SRM_REPORT
/*
  dump pictures occuping the SRM
*/
EbErrorType dump_srm_content(EbSystemResource* resource_ptr, uint8_t log);
#endif
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
EbErrorType svt_post_full_object(EbObjectWrapper* object_ptr);

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
EbErrorType svt_get_full_object(EbFifo* full_fifo_ptr, EbObjectWrapper** wrapper_dbl_ptr);

EbErrorType svt_get_full_object_non_blocking(EbFifo* full_fifo_ptr, EbObjectWrapper** wrapper_dbl_ptr);

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
EbErrorType svt_release_object(EbObjectWrapper* object_ptr);

/*********************************************************************
     * svt_shutdown_process
     *   Notify shut down signal to consumer of EbSystemResource.
     *   So that the consumer process can break the loop and quit running.
     *
     *   resource_ptr
     *      pointer to the SystemResource.
     *********************************************************************/
EbErrorType svt_shutdown_process(const EbSystemResource* resource_ptr);

#define EB_GET_FULL_OBJECT(full_fifo_ptr, wrapper_dbl_ptr)                     \
    do {                                                                       \
        EbErrorType err = svt_get_full_object(full_fifo_ptr, wrapper_dbl_ptr); \
        if (err == EB_NoErrorFifoShutdown)                                     \
            return NULL;                                                       \
    } while (0)

#ifdef __cplusplus
}
#endif
#endif //EbSystemResource_h
