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
 * @file FrameQueue.cc
 *
 * @brief Impelmentation of reconstructed frame queue
 *
 ******************************************************************************/

#include <stdio.h>
#include <vector>
#include <algorithm>
#include "FrameQueue.h"
#include "CompareTools.h"
#ifdef ENABLE_DEBUG_MONITOR
#include "VideoMonitor.h"
#endif

#if _WIN32
#define fseeko _fseeki64
#define ftello _ftelli64
#define FOPEN(f, s, m) fopen_s(&f, s, m)
#else
#define fseeko fseek
#define ftello ftell
#define FOPEN(f, s, m) f = fopen(s, m)
#endif

using svt_av1_e2e_tools::compare_image;

bool FrameQueue::compare(FrameQueue *other) {
    if (frame_count_ != other->get_frame_count()) {
        printf("frame count(%u<-->%u) are different",
               frame_count_,
               other->get_frame_count());
        return false;
    }
    bool is_same = true;
    for (uint32_t i = 0; i < frame_count_; i++) {
        VideoFrame *frame = take_frame_inorder(i);
        VideoFrame *other_frame = other->take_frame_inorder(i);
        is_same = compare_image(frame, other_frame);
        if (!is_same) {
            printf("ref_frame(%u) compare failed!!\n",
                   (uint32_t)frame->timestamp);
            break;
        }
    }

    return is_same;
}

class FrameQueueFile : public FrameQueue {
  public:
    FrameQueueFile(const VideoFrameParam &param, const char *file_path)
        : FrameQueue(param) {
        queue_type_ = FRAME_QUEUE_FILE;
        max_frame_ts_ = 0;
        FOPEN(recon_file_, file_path, "wb");
        record_list_.clear();
    }
    FrameQueueFile(const FrameQueueFile &) = delete;
    FrameQueueFile &operator=(const FrameQueueFile &) = delete;

    virtual ~FrameQueueFile() {
        if (recon_file_) {
            fflush(recon_file_);
            fclose(recon_file_);
            recon_file_ = nullptr;
        }
        record_list_.clear();
    }
    void add_frame(VideoFrame *frame) override {
        if (recon_file_ && frame->buf_size &&
            frame->timestamp < (uint64_t)frame_count_) {
            if (frame->timestamp >=
                max_frame_ts_) {  // new frame is larger than max timestamp
                fseeko(recon_file_, 0, SEEK_END);
                for (size_t i = max_frame_ts_; i < frame->timestamp + 1; ++i) {
                    fwrite(frame->buffer, 1, frame->buf_size, recon_file_);
                }
                max_frame_ts_ = frame->timestamp;
            }

            rewind(recon_file_);
            uint64_t frame_num = frame->timestamp;
            while (frame_num > 0) {
                int ret = fseeko(recon_file_, frame->buf_size, SEEK_CUR);
                if (ret != 0) {
                    return;
                }
                frame_num--;
            }
            fwrite(frame->buffer, 1, frame->buf_size, recon_file_);
            fflush(recon_file_);
            record_list_.push_back((uint32_t)frame->timestamp);
        }
        delete frame;
    }
    VideoFrame *take_frame(const uint64_t time_stamp) override {
        if (recon_file_ == nullptr)
            return nullptr;

        VideoFrame *new_frame = nullptr;
        fseeko(recon_file_, 0, SEEK_END);
        int64_t actual_size = ftello(recon_file_);
        if (actual_size > 0 &&
            ((uint64_t)actual_size) > ((time_stamp + 1) * frame_size_)) {
            int ret = fseeko(recon_file_, time_stamp * frame_size_, 0);
            if (ret != 0) {
                return nullptr;
            }
            new_frame = get_empty_frame();
            if (new_frame) {
                size_t read_size =
                    fread(new_frame->buffer, 1, frame_size_, recon_file_);
                if (read_size != frame_size_) {
                    printf("read recon file error!\n");
                    delete_frame(new_frame);
                    new_frame = nullptr;
                } else {
                    new_frame->timestamp = time_stamp;
                }
            }
        }

        return new_frame;
    }
    VideoFrame *take_frame_inorder(const uint32_t index) override {
        return take_frame(index);
    }
    void recycle_frame(VideoFrame *frame) override {
        delete_frame(frame);
    }
    void delete_frame(VideoFrame *frame) override {
        delete frame;
    }

  public:
    FILE *recon_file_; /**< file handle to dave reconstructed video frames, set
                          it to public for accessable by create_frame_queue */

  protected:
    uint64_t max_frame_ts_; /**< maximun timestamp of current frames in list */
    std::vector<uint32_t> record_list_; /**< list of frame timstamp, to help
                                           check if the file is completed*/
};

class FrameQueueBufferSort_ASC {
  public:
    bool operator()(const VideoFrame *a, const VideoFrame *b) const {
        return a->timestamp < b->timestamp;
    }
};

class FrameQueueBuffer : public FrameQueue {
  public:
    explicit FrameQueueBuffer(const VideoFrameParam &fmt) : FrameQueue(fmt) {
        queue_type_ = FRAME_QUEUE_BUFFER;
        frame_list_.clear();
    }
    virtual ~FrameQueueBuffer() {
        while (frame_list_.size() > 0) {
            delete frame_list_.back();
            frame_list_.pop_back();
        }
    }
    void add_frame(VideoFrame *frame) override {
        if (frame->timestamp < (uint64_t)frame_count_) {
            frame_list_.push_back(frame);
            std::sort(frame_list_.begin(),
                      frame_list_.end(),
                      FrameQueueBufferSort_ASC());
        } else  // drop the frames out of limitation
            delete frame;
    }
    VideoFrame *take_frame(const uint64_t time_stamp) override {
        auto ret = std::find_if(frame_list_.begin(),
                                frame_list_.end(),
                                [time_stamp](VideoFrame *frame) {
                                    return frame->timestamp == time_stamp;
                                });
        return (ret != frame_list_.end()) ? *ret : nullptr;
    }
    VideoFrame *take_frame_inorder(const uint32_t index) override {
        if (index < frame_list_.size())
            return frame_list_.at(index);
        return nullptr;
    }
    void recycle_frame(VideoFrame *frame) override {
        frame->trim_buffer();
    }
    void delete_frame(VideoFrame *frame) override {
        std::vector<VideoFrame *>::iterator it =
            std::find(frame_list_.begin(), frame_list_.end(), frame);
        if (it != frame_list_.end()) {  // if the video frame is in list
            delete *it;
            frame_list_.erase(it);
        } else  // only delete the video frame not in list
            delete frame;
    }

  protected:
    std::vector<VideoFrame *> frame_list_; /**< list of frame containers */
};

class RefQueue : public ICompareQueue, FrameQueueBuffer {
  public:
    RefQueue(VideoFrameParam fmt, FrameQueue *my_friend)
        : FrameQueueBuffer(fmt) {
        friend_ = my_friend;
        frame_vec_.clear();
#ifdef ENABLE_DEBUG_MONITOR
        recon_monitor_ = nullptr;
        ref_monitor_ = nullptr;
#endif
    }
    virtual ~RefQueue() {
        while (frame_vec_.size()) {
            const VideoFrame *p = frame_vec_.back();
            frame_vec_.pop_back();
            if (p) {
                delete p;
            }
        }
        friend_ = nullptr;
#ifdef ENABLE_DEBUG_MONITOR
        if (recon_monitor_) {
            delete recon_monitor_;
            recon_monitor_ = nullptr;
        }
        if (ref_monitor_) {
            delete ref_monitor_;
            ref_monitor_ = nullptr;
        }
#endif
    }

  public:
    bool compare_video(VideoFrame &frame) override {
        VideoFrame *friend_frame = friend_->take_frame(frame.timestamp);
        if (friend_frame) {
            draw_frames(&frame, friend_frame);
            bool is_same = compare_image(friend_frame, &frame);
            if (!is_same) {
                printf("ref_frame(%u) compare failed!!\n",
                       (uint32_t)frame.timestamp);
            }
            friend_->recycle_frame(friend_frame);
            return is_same;
        } else {
            clone_frame(frame);
        }
        return true; /**< default return suceess if not found recon frame */
    }
    bool flush_video() override {
        bool is_all_same = true;
        for (VideoFrame *frame : frame_vec_) {
            VideoFrame *friend_frame = friend_->take_frame(frame->timestamp);
            if (friend_frame) {
                draw_frames(frame, friend_frame);
                if (!compare_image(friend_frame, frame)) {
                    printf("ref_frame(%u) compare failed!!\n",
                           (uint32_t)frame->timestamp);
                    is_all_same = false;
                }
                friend_->recycle_frame(friend_frame);
            }
        }
        return is_all_same;
    }

  private:
    void clone_frame(const VideoFrame &frame) {
        VideoFrame *new_frame = new VideoFrame(frame);
        if (new_frame)
            frame_vec_.push_back(new_frame);
        else
            printf("out of memory for clone video frame!!\n");
    }
    void draw_frames(const VideoFrame *frame, const VideoFrame *friend_frame) {
#ifdef ENABLE_DEBUG_MONITOR
        if (ref_monitor_ == nullptr) {
            /** walk-around for bit-depth is fixed set 10-bit from ref-decoder,
             * here to use bit-depth of friend frame*/
            ref_monitor_ = new VideoMonitor(frame->width,
                                            frame->height,
                                            frame->stride[0],
                                            friend_frame->bits_per_sample,
                                            false,
                                            "Ref decode");
        }
        if (ref_monitor_) {
            ref_monitor_->draw_frame(
                frame->planes[0], frame->planes[1], frame->planes[2]);
        }
        // Output to monitor for debug
        if (recon_monitor_ == nullptr) {
            recon_monitor_ = new VideoMonitor(friend_frame->width,
                                              friend_frame->height,
                                              friend_frame->stride[0],
                                              friend_frame->bits_per_sample,
                                              false,
                                              "Recon");
        }
        if (recon_monitor_) {
            recon_monitor_->draw_frame(friend_frame->planes[0],
                                       friend_frame->planes[1],
                                       friend_frame->planes[2]);
        }
#else
        (void)frame;
        (void)friend_frame;
#endif
    }

  private:
    FrameQueue *friend_;
    std::vector<VideoFrame *> frame_vec_;
#ifdef ENABLE_DEBUG_MONITOR
    VideoMonitor *recon_monitor_;
    VideoMonitor *ref_monitor_;
#endif
};

FrameQueue *create_frame_queue(const VideoFrameParam &param,
                               const char *file_path) {
    FrameQueueFile *new_queue = new FrameQueueFile(param, file_path);
    if (new_queue) {
        if (new_queue->recon_file_ == nullptr) {
            delete new_queue;
            new_queue = nullptr;
        }
    }
    return new_queue;
}

FrameQueue *create_frame_queue(const VideoFrameParam &param) {
    return new FrameQueueBuffer(param);
}

ICompareQueue *create_ref_compare_queue(const VideoFrameParam &param,
                                        FrameQueue *recon) {
    return new RefQueue(param, recon);
}
