#!/usr/bin/env python3
# Copyright(c) 2025 Meta Platforms, Inc. and affiliates.
#
# This source code is subject to the terms of the BSD 2 Clause License and
# the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
# was not distributed with this source code in the LICENSE file, you can
# obtain it at https://www.aomedia.org/license/software-license. If the
# Alliance for Open Media Patent License 1.0 was not distributed with this
# source code in the PATENTS file, you can obtain it at
# https://www.aomedia.org/license/patent-license.

import os
import re
import shutil
import subprocess
import time
from multiprocessing import cpu_count


# read spec from very end of file name
src_re = r"(?P<name>.+)_(?P<width>\d+)x(?P<height>\d+)(?:_(?P<fps>\d+))?"


def create_logger(name, path):
    import logging

    os.makedirs(os.path.dirname(path), exist_ok=True)

    logger = logging.getLogger(name)
    for handler in logger.handlers[:]:
        logger.removeHandler(handler)

    log_handler = logging.FileHandler(path, "w")
    log_handler.setLevel(logging.INFO)
    log_handler.setFormatter(
        logging.Formatter("[%(asctime)s][%(levelname)s] %(message)s")
    )
    logger.addHandler(log_handler)
    logger.setLevel(logging.DEBUG)

    return logger


def delete_file(fpath):
    if os.path.exists(fpath):
        os.remove(fpath)


def clean_directory(directory):
    # purge entire dir
    if os.path.isdir(directory):
        try:
            shutil.rmtree(directory)
        except Exception as e:
            print(f"Failed to delete {directory}. Reason: {e}")

    # re-create a path
    os.makedirs(directory, exist_ok=True)


def get_file_desc(fn):
    pattern = re.compile(src_re)
    match = pattern.search(fn)
    if match:
        width = int(match.group("width"))
        height = int(match.group("height"))
        fps = int(match.group("fps")) if match.group("fps") else 30
        return width, height, fps
    return None, None, None


def quality_to_kbps(width, height, fps, quality):
    if width and height and fps:
        return int(width * height * fps * 1.5 * 8 / (quality * 1000))

    return 0


def get_original_file_name(filename):
    fn, ext = os.path.splitext(filename)
    pattern = re.compile(src_re)
    match = pattern.search(fn)
    if match:
        name = match.group("name")
        return name + ext
    return filename


def get_max_workers(max_workers: int) -> int:
    if max_workers < 1:
        # leave some CPU power to the system
        max_workers = max(1, int(cpu_count() * 0.75), cpu_count() - 8)
    return min(max_workers, int(cpu_count() * 2))


def get_cmd_times(cmd, passes=1):
    """
    Execute command and return its execution time.

    Args:
        cmd: command line to run, as single string for shell=True usage
        passes: number of iterations

    Returns:
        Process time in seconds
    """

    # use system `time` command in POSIX format
    time_cmd = "/usr/bin/time -p"

    total_time = 0.0
    wallclock_time = 0.0
    actual_passes = 0

    if passes > 0:
        # for fixed number of passes - do all at once
        min_time_sec = 100000  # no time limit
        max_passes = passes
    else:
        # if passes is < 1, ie auto, run command for at least this amount of time
        # this increases time measurements accuracy, as time accuracy is 10 ms
        # number of passes double on each iteration
        min_time_sec = 0.2  # run for minimum 200 ms
        max_passes = 32 * 2 - 1  # or maximum 32 consecutive times
        passes = 1

    time_s = time.time()
    do_one_more = True
    while do_one_more:
        run_cmd = (
            time_cmd
            + ' /bin/sh -c "for i in {1..'
            + str(passes)
            + "}; do "
            + cmd
            + '; done"'
        )

        res = subprocess.run(run_cmd, shell=True, capture_output=True, text=True)

        try:
            time_values = res.stderr.splitlines()[-3:]
            usr_time = float(time_values[1].split()[-1])
            sys_time = float(time_values[2].split()[-1])
            total_time += usr_time + sys_time
            actual_passes += passes
            do_one_more = actual_passes < max_passes and total_time < min_time_sec
            passes *= 2
        except Exception as e:
            print(f"Failed to measure time: {e}")
            do_one_more = False

    wallclock_time = time.time() - time_s

    if actual_passes == 0:
        return wallclock_time

    use_usr_sys_time = True
    if use_usr_sys_time:
        return total_time / actual_passes

    return wallclock_time / actual_passes
