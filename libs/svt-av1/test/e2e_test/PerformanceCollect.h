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
 * @file PerformanceCollect.h
 *
 * @brief Defines a performance tool for timing collection
 *
 * @author Cidana-Edmond
 *
 ******************************************************************************/

#ifndef _PERFORMANCE_COLLECT_H_
#define _PERFORMANCE_COLLECT_H_

#include <algorithm>
#include <stdint.h>
#include <string>
#include <vector>

/** PerformanceCollect is a class designed to collect time in test for a report
of performance evaluation*/
class PerformanceCollect {
  public:
    /** Collector is a structure to store and count the time of test item */
    typedef struct Collector {
        uint64_t init_tick;       /**< time tick of this collect created */
        uint64_t last_start_tick; /**< time tick of last start of counting */
        uint64_t count_ticks;     /**< time ticks sum of every counting */
        const std::string name;   /**< name of test item */
        Collector(const std::string &n, const uint64_t unite_tick)
            : init_tick(unite_tick ? unite_tick : get_time_tick()),
              last_start_tick(0),
              count_ticks(0),
              name(n) {
        }
        /** tag_once() ends a counting of and save this perid of time in sum */
        void tag_once() {
            if (last_start_tick > 0) {
                count_ticks += (get_time_tick() - last_start_tick);
                last_start_tick = 0;
            }
        }
        /** to_string() can return a string show the time of item cost, and its
         * CPU usage in whole test
         * @return
         * string of time cost and CPU usage
         */
        std::string to_string() {
            uint64_t total_ticks = get_time_tick() - init_tick;
            std::string print =
                "[" + name + "] cost: " + std::to_string(count_ticks) + "ms, ";
            print +=
                "usage: " + std::to_string(count_ticks * 100 / total_ticks) +
                "%\n";
            return print;
        }
    } *CollectHandle;

  public:
    /** Constructor of PerformanceCollect
     * @param test_name the name of test case
     */
    explicit PerformanceCollect(const std::string &test_name) {
        init_tick_ = get_time_tick();
        test_name_ = test_name;
        collect_vec_.clear();
    }
    /** Destructor of PerformanceCollect */
    virtual ~PerformanceCollect() {
        while (collect_vec_.size()) {
            CollectHandle p = collect_vec_.back();
            collect_vec_.pop_back();
            printf("%s%s", test_name_.c_str(), p->to_string().c_str());
        }
    }
    /** Start counting time of specificate test item
     * @param item_name the name of test item
     * @return
     * CollectHandle -- the handle of time collecter <br>
     * nullptr -- can not create a collector
     */
    CollectHandle start_count(const std::string &item_name) {
        auto collector_handle =
            std::find_if(collect_vec_.begin(),
                         collect_vec_.end(),
                         [&item_name](CollectHandle p) {
                             return p->name.compare(item_name) == 0;
                         });

        CollectHandle collector = nullptr;
        if (collector_handle == collect_vec_.end()) {
            collector = new Collector(item_name, init_tick_);
            collect_vec_.push_back(collector);
        } else {
            collector = *collector_handle;
        }
        if (collector->last_start_tick) {
            printf("last counting is not stopped, skip this attampt!!\n");
        } else
            collector->last_start_tick = get_time_tick();
        return collector;
    }
    /** Stop counting time
     * @param collector the handle of time collector
     */
    void stop_count(const CollectHandle collector) {
        if (collector) {
            collector->tag_once();
        }
    }
    /** Read the total counting time
     * @param item_name the name of test item
     * @return
     * the perid of time in counting
     */
    uint64_t read_count(const std::string &item_name) {
        auto ret = std::find_if(collect_vec_.begin(),
                                collect_vec_.end(),
                                [&item_name](CollectHandle p) {
                                    return p->name.compare(item_name) == 0;
                                });
        return ret != collect_vec_.end() ? (*ret)->count_ticks : 0;
    }

  private:
    /** Get currect system time tick
     * @return
     * time tick of system clock in milliseconds
     */
    static uint64_t get_time_tick();

  protected:
    std::string test_name_;                  /**< name of test case*/
    std::vector<CollectHandle> collect_vec_; /**< vector of collectors */
    uint64_t init_tick_; /**< the unified initialization for all collectors */
};

/** TimeAutoCount is a tool class designed for much easier to use
 * PerformanceCollect */
class TimeAutoCount {
  public:
    TimeAutoCount(const std::string &name, PerformanceCollect *counter)
        : counter_(counter) {
        collect_ = nullptr;
        if (counter_) {
            collect_ = counter_->start_count(name);
        }
    }
    ~TimeAutoCount() {
        counter_->stop_count(collect_);
    }

  private:
    PerformanceCollect *counter_; /**< context of performance collection */
    PerformanceCollect::CollectHandle collect_; /**< collect in use */
};
#endif  // !_PERFORMANCE_COLLECT_H_
