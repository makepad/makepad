/*
 * Copyright(c) 2019 Netflix, Inc.
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the
 * Alliance for Open Media Patent License 1.0 was not distributed with this
 * source code in the PATENTS file, you can obtain it at
 * https://www.aomedia.org/license/patent-license.
 */

/******************************************************************************
 * @file FrameQueue.h
 *
 * @brief Defines a queue to collect reconstructed frames
 *
 ******************************************************************************/

#ifndef _FRAME_QUEUE_H_
#define _FRAME_QUEUE_H_

#include <stdint.h>
#include <memory.h>
#include "VideoFrame.h"

/** FrameQueue is a class designed to collect YUV video frames. It provides
 * interfaces for generating, store and destroy frame containers. It can be
 * implemented with file-mode or buffer-mode to store the video frames, and it
 * also provides inside sorting by timestamp.
 */
class FrameQueue {
  public:
    /** FrameQueueType is enumerate type of queue type, file or buffer mode */
    typedef enum FrameQueueType {
        FRAME_QUEUE_BUFFER,
        FRAME_QUEUE_FILE,
    } FrameQueueType;

  public:
    /** Constructor of FrameQueue
     * @param param the parameters of the video frame
     */
    explicit FrameQueue(const VideoFrameParam& param)
        : queue_type_(FRAME_QUEUE_BUFFER),
          video_param_(param),
          frame_size_(VideoFrame::calculate_max_frame_size(param)),
          frame_count_(0) {
    }
    /** Destructor of FrameQueue      */
    virtual ~FrameQueue() {
    }
    /** Get queue type
     * @return
     * FrameQueueType -- the type of queue
     */
    FrameQueueType get_type() {
        return queue_type_;
    }
    /** Get video parameter
     * @return
     * VideoFrameParam -- the parameter of video frame
     */
    VideoFrameParam get_video_param() {
        return video_param_;
    }
    /** Get total frame count in queue
     * @return
     * uint32_t -- the count of frame in queue
     */
    uint32_t get_frame_count() {
        return frame_count_;
    }
    /** Get maximum video frame number in queue
     * @param count  the maximum video frame number
     */
    void set_frame_count(const uint32_t count) {
        frame_count_ = count;
    }
    /** Get an empty video frame from queue
     * @return
     * VideoFrame -- a container of video frame
     * nullptr -- no available video frame
     */
    VideoFrame* get_empty_frame() {
        return new VideoFrame(video_param_);
    }
    /** Interface of insert a video frame into queue
     * @param frame  the video frame to insert into queue
     */
    virtual void add_frame(VideoFrame* frame) = 0;
    /** Interface of get a video frame by the same timestamp
     * @param time_stamp  the timestamp of video frame to retreive
     * @return
     * VideoFrame -- a container of video frame
     * nullptr -- no available video frame by this timestamp
     */
    virtual VideoFrame* take_frame(const uint64_t time_stamp) = 0;
    /** Interface of get a video frame by index
     * @param index  the index of video to retreive
     * @return
     * VideoFrame -- a container of video frame
     * nullptr -- no available video frame by index
     */
    virtual VideoFrame* take_frame_inorder(const uint32_t index) = 0;
    /** Interface of recycle a video frame with caculate its checksum and free
     * the memory of buffer
     * @param frame  the video frame to recycle
     */
    virtual void recycle_frame(VideoFrame* frame) = 0;
    /** Interface of destroy a video frame and remove from queue
     * @param frame  the video frame to distroy
     */
    virtual void delete_frame(VideoFrame* frame) = 0;
    /** Interface of compare with other frame queue
     * @param other  other frame queue to compare
     * @return
     * true -- the queue is same
     * false -- the queue is different
     */
    virtual bool compare(FrameQueue* other);

  protected:
    FrameQueueType queue_type_;   /**< type of queue*/
    VideoFrameParam video_param_; /**< video frame parameters*/
    uint32_t frame_size_;         /**< size of video frame*/
    uint32_t frame_count_;        /**< maximun number of video frames*/
};

class ICompareQueue {
  public:
    virtual ~ICompareQueue() {};
    virtual bool compare_video(VideoFrame& frame) = 0;
    virtual bool flush_video() = 0;
};

/** Interface of create a queue of reconstructed video frame with video
 * parameters and the file path to store
 * @param param  the parameter of video frame
 * @param file_path  the file path to store the containers
 * @return
 * FrameQueue -- the queue created
 * nullptr -- creation failed
 */
FrameQueue* create_frame_queue(const VideoFrameParam& param,
                               const char* file_path);

/** Interface of create a queue of reconstructed video frame with video
 * parameters
 * @param param  the parameter of video frame
 * @return
 * FrameQueue -- the queue created
 * nullptr -- creation failed
 */
FrameQueue* create_frame_queue(const VideoFrameParam& param);

/** Interface of create a queue of reference frames to compare with recon
 * parameters
 * @param param  the parameter of video frame
 * @param recon  the queue of recon video frame
 * @return
 * FrameQueue -- the queue created
 * nullptr -- creation failed
 */
ICompareQueue* create_ref_compare_queue(const VideoFrameParam& param,
                                        FrameQueue* recon);

#endif  // !_FRAME_QUEUE_H_
