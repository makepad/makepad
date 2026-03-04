#!/usr/bin/env python3

"""
This script is used to replicate CI jobs locally.

Requires:

- Python 3.8+
- python-yaml or python-pyyaml
- Docker with a working internet connection
- Git

Check the package manager for your distro for the correct package names.
"""


import asyncio
import json
import shutil
import tarfile
from asyncio import (Event, Queue, Task, create_subprocess_exec, create_task,
                     sleep)
from asyncio.subprocess import DEVNULL, PIPE, STDOUT, Process
from datetime import datetime
from enum import Enum
from io import IOBase
from os import environ

try:
    from os import getgid, getuid
except ImportError:
    def getgid():
        """Return 0 on non-linux"""
        return 0

    def getuid():
        """Return 0 on non-linux"""
        return 0
from ast import literal_eval
from pathlib import Path
from shlex import quote as shquote
from sys import platform, stderr, version_info
from time import monotonic
from typing import Any, Dict, List, Set, Tuple, Union
from urllib import request
from urllib.parse import quote as urlquote

try:
    import yaml
except ImportError:
    print("Please install python-yaml or python-pyyaml", file=stderr)
    exit(1)

# Global Variables
DEFAULT_PROJECT_NAME: str = "AOMediaCodec/SVT-AV1"


# Don't modify past here
# ---------------------
GLOBAL_VARS: dict = {'CMAKE_GENERATOR': 'Ninja'}
VAR_OVERRIDES: dict = {'FFMPEG_CONFIG_FLAGS': '--disable-doc'}
DOCKER_REPO: str = "registry.gitlab.com/aomediacodec/aom-testing"
DOCKER_HELPER_IMAGE: str = "alpine"
REPO_DIR = Path(__file__).parent.parent.resolve()
BASE_DIR = REPO_DIR / "offline_ci"
FAILED_LOG_DIR = BASE_DIR / "failed_logs"
RUNNING_LOG_DIR = BASE_DIR / "running_logs"
FINISHED_LOG_DIR = BASE_DIR / "finished_logs"
ARTIFACT_DIR = BASE_DIR / "artifacts"
SCRIPT_LOG = BASE_DIR / "offline_ci.log"
PREVIOUS_JOB = BASE_DIR / "previous_job"
RUNNING: Set[str] = set()  # list of running jobs
FAILED: Set[str] = set()  # list of failed jobs
FINISHED: Set[str] = set()  # list of finished jobs
QUEUED: Set[str] = set()  # list of queued jobs
MAX_RUNNERS: int = 5


class OS(Enum):
    """Enum for OS"""
    UNKNOWN = "unknown"
    LINUX = "linux"
    WINDOWS = "windows"
    MACOS = "macos"


class Job(Dict):
    """Job class"""

    class State(Enum):
        """Enum for Job State"""
        PENDING = "pending"
        RUNNING = "running"
        FINISHED = "finished"
        FAILED = "failed"

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.event = Event()
        self["dependents"]: Dict[Job] = {}
        self["needs"]: Dict[Job] = {}
        self["image"] = ""
        self.artifacts = []
        self["variables"] = {}
        self.target_os: OS = OS.UNKNOWN

    def __hash__(self):
        return hash(self["name"])

    def __eq__(self, other):
        return self["name"] == other["name"]


JOBS: dict[str, Job] = {}


HOST_OS: OS = OS.WINDOWS if platform.startswith(
    "win") else OS.MACOS if platform.startswith("darwin") else OS.LINUX


def dependency_checks():
    """Check for dependencies"""
    failed: bool = False
    if version_info < (3, 8):
        print("Please use Python 3.8+", file=stderr)
        failed = True
    if HOST_OS in (OS.WINDOWS, OS.LINUX) and shutil.which("docker") is None:
        print("Please install docker", file=stderr)
        failed = True
    if shutil.which("git") is None:
        print("Please install git", file=stderr)
        failed = True
    if failed:
        exit(1)


def get_api_url(project_name: str) -> str:
    """Return the API url for a specific project"""
    return f"https://gitlab.com/api/v4/projects/{urlquote(project_name, safe='')}"


def pull_json(project_name: str) -> dict:
    """Pull the json from the API"""
    json_cache: Path = BASE_DIR / "cache.json"
    json_cache.parent.mkdir(parents=True, exist_ok=True)
    # if the cache is older than a day, delete it
    if json_cache.exists() and \
            (datetime.now() - datetime.fromtimestamp(json_cache.stat().st_mtime)).days > 1:
        json_cache.unlink()
    req = request.Request(get_api_url(project_name) +
                          "/ci/lint?include_merged_yaml=true&include_jobs=true")
    if "HTTP_PROXY" in environ:
        req.set_proxy(environ["HTTP_PROXY"], "http")
    if "HTTPS_PROXY" in environ:
        req.set_proxy(environ["HTTPS_PROXY"], "https")
    with json_cache.open("rb") if json_cache.exists() else request.urlopen(req) as response:
        if not json_cache.exists():
            json_cache.write_bytes(response.read())
        return json.loads(json_cache.read_text())


class DevNullLike(IOBase):
    """DevNullLike class"""

    def write(self, _):
        """Write to devnull"""
        pass

    def writelines(self, _):
        """Write lines to devnull"""
        pass

    def flush(self):
        """Flush devnull"""
        pass

    def fileno(self):
        """Return the file number"""
        return -3


async def run_subprocess(prog, *args: List, log: IOBase = None, **kwargs) -> Process:
    """Run a subprocess"""
    if log is None:
        log = DevNullLike()
    print("Running", file=log, flush=True)
    print(
        f"{' '.join(['$ ' + shquote(str(prog))] + [shquote(str(arg)) for arg in args])}",
        file=log, flush=True)
    print(f"with kwargs {kwargs}", file=log, flush=True)

    if "stdout" not in kwargs:
        if isinstance(log, DevNullLike):
            kwargs["stdout"] = DEVNULL
        else:
            kwargs["stdout"] = log
    if "stderr" not in kwargs:
        kwargs["stderr"] = STDOUT
    return await create_subprocess_exec(prog, *args, **kwargs)


def handle_job_keys(job: dict, ret: Job):
    """Handle the keys in a job"""
    atts_to_exclude: List[str] = ["stage", "tag_list", "only",
                                  "except", "environment", "when", "allow_failure"]
    for key, value in job.items():
        if key in atts_to_exclude:
            continue
        ret[key] = value


def expand_dict(input_dict: Dict[str, Union[list, str, int]]) -> List[Dict[str, Union[str, int]]]:
    """Expand a dict for variables"""
    ret: List[Dict[str, Union[str, int]]] = []
    num_lists = [isinstance(value, list)
                 for value in input_dict.values()].count(True)
    if num_lists == 0:
        # simple case
        ret_to_add = {}
        for key, value in input_dict.items():
            ret_to_add[key] = value
        ret.append(ret_to_add)
        return ret
    # somewhat simple case of single var, single list
    if len(input_dict) == 1:
        key, value = list(input_dict.items())[0]
        for value in value:
            ret.append({key: value})
        return ret
    # more complex case of multiple vars, multiple lists
    # We need to matrix the lists
    # for k1, v1 in a.items():
    #     for k2, v2 in b.items():
    #         for k3, v3 in c.items():
    #             for val1 in v1:
    #                 for val2 in v2:
    #                     for val3 in v3:
    #                         {k1: val1, k2: val2, k3: val3}
    # where a, b, and c are dicts
    # and v1, v2, and v3 are lists

    # First, we need to get the lists
    lists: List[Tuple[str, List[Any]]] = []
    for key, value in input_dict.items():
        if isinstance(value, list):
            lists.append((key, value))
    # Now, we need to matrix the lists
    # We do this by taking the first list, and for each item in the list, we create a new dict with
    # that item, and then we recurse on the rest of the lists
    key, value = lists[0]
    for item in value:
        ret.extend(expand_dict(
            {key: item, **{k: v for k, v in input_dict.items() if k != key}}))
    # and hope that we're done
    return ret


def expand_matrix(matrix: List[Dict[str, Union[list, str, int]]]
                  ) -> List[Dict[str, Union[str, int]]]:
    """Expand a matrix"""
    ret: List[Dict[str, Union[str, int]]] = []
    for item in matrix:
        ret.extend(expand_dict(item))
    return ret


def detect_os(yml_job: Dict) -> OS:
    """Detect the OS of a job"""
    tags: List[str] = yml_job.get("tags", [])
    if 'svt-windows-docker' in tags:
        return OS.WINDOWS
    if 'macos-ci' in tags or 'macos' in tags:
        return OS.MACOS
    return OS.LINUX


def setup_job(job: dict, yml: dict) -> Job:
    """Setup a job from the json and yml"""
    ret: Job = Job()
    name: str = job["name"]
    yml_job: dict = yml[name.split(':')[0]]

    ret.target_os = detect_os(yml_job)

    handle_job_keys(job, ret)

    ret["dependents"] = {}
    if ret["needs"] is None:
        ret["needs"] = {}
    image = str(yml_job.get("image", ""))
    if image.startswith(DOCKER_REPO + '/'):
        image = image[len(DOCKER_REPO + '/'):]
    ret["image"] = image
    if 'artifacts' in yml_job.keys() and 'paths' in yml_job["artifacts"].keys():
        ret.artifacts.extend(yml_job["artifacts"]["paths"])

    ret["variables"] = {}
    ret["variables"].update(GLOBAL_VARS)
    ret["variables"].update(yml_job.get("variables", {}))
    for key, value in VAR_OVERRIDES.items():
        if key in ret["variables"]:
            ret["variables"][key] = value
    if not 'parallel' in yml_job.keys() or ':' not in name:
        return ret

    # Then, for each of those, try to find a match from matrix_values
    # to get the correct variable names
    yml_matrix: List[Dict[str, Union[List[str, int], str, int]]
                     ] = yml_job["parallel"]["matrix"]
    vars_str = name.split(':')[1].strip()
    vars_str = vars_str[1 if vars_str.startswith('[') else 0:
                        -1 if vars_str.endswith(']') else len(vars_str)]
    matrix_values: List[str] = [splitted.strip()
                                for splitted in vars_str.split(', ')]

    # First, we need to expand the matrix
    yml_matrix = expand_matrix(yml_matrix)

    # Then, we need to find the correct matrix
    # We do this by checking if each matrix is possible
    # If it is, we use it
    # If it isn't, we move on to the next one
    # If we run out of matrices, we error
    # A matrix is possible if all of the values are in matrix_values
    # If a matrix is possible, we update the variables with the values
    # from the matrix
    for matrix in yml_matrix:
        possible = True
        for key, value in matrix.items():
            if str(value) not in matrix_values:
                possible = False
                break
        if possible:
            ret["variables"].update(matrix)
            break
    else:
        raise ValueError(f"Unable to find matrix for job {name}")

    return ret


def generate_dependentent_set(job_name: str) -> Set[str]:
    """Generate a set of all jobs that depend on the given job"""
    ret: Set[str] = set()
    if job_name not in JOBS:
        return ret
    for dependent in JOBS[job_name]["dependents"]:
        ret.add(dependent)
        ret.update(generate_dependentent_set(dependent))
    return ret


def read_json(project_name: str = DEFAULT_PROJECT_NAME) -> dict:
    """Pulls the json from the API, should be called only once at the beginning of the program"""
    j = pull_json(project_name)
    if not j["valid"]:
        return {}
    yml: dict = yaml.safe_load(j["merged_yaml"])
    if 'variables' in yml.keys():
        GLOBAL_VARS.update(yml["variables"])
    for job in j["jobs"]:
        name: str = job["name"]
        JOBS[name] = setup_job(job, yml)

    for job in JOBS.values():
        needs = job["needs"]
        job["needs"] = {}
        if not needs:
            continue
        deps: Dict = {}
        for need in needs:
            name = need["name"]
            deps[name] = JOBS[name]
            JOBS[name]["dependents"][job["name"]] = job
        job["needs"].update(deps)

    # We need to exclude jobs that are not available to this machine,
    # and all of their dependents
    job_to_remove: Set[str] = set()
    for job in list(JOBS.keys()):
        if JOBS[job].target_os != HOST_OS:
            job_to_remove.add(job)
            job_to_remove.update(generate_dependentent_set(job))

    for job in job_to_remove:
        JOBS.pop(job)
    return JOBS


def write_script_sh(file: Path, before: List[str], script: List[str], after: List[str],
                    base_script: List[str]):
    """Write out the scripts for a job for sh"""
    for suff, content in [('_before_script', before), ('_script', script),
                          ('_after_script', after), ('', base_script)]:
        path = Path(str(file) + suff + '.sh')
        path.write_text("#!/usr/bin/sh\n"
                        "set -ex\n" + "\n".join(content), encoding="utf-8")
        path.chmod(0o755)


def write_script_ps1(file: Path, before: List[str], script: List[str], after: List[str],
                     base_script: List[str]):
    """Write out the scripts for a job for powershell"""
    for suff, content in [('_before_script', before), ('_script', script),
                          ('_after_script', after), ('', base_script)]:
        path = Path(str(file) + suff + '.ps1')
        path.write_text("#!/usr/bin/env pwsh\n" +
                        "\n".join(content), encoding="utf-8")
        path.chmod(0o755)


def write_script(job: dict, safe_name: str, project_dir: Path, variables: Dict) -> List[str]:
    """Write out the scripts for a job, returns the command to run"""
    outside_dir = Path(project_dir)
    inside_dir = Path("/") / REPO_DIR.name
    inside_git_dir = inside_dir / ".git"

    before_script = job["before_script"]
    script = job["script"]
    after_script = job["after_script"]
    outside_script_base = outside_dir / "scripts" / safe_name
    outside_script_base.parent.mkdir(parents=True, exist_ok=True)
    inside_script_base = inside_dir / "scripts" / safe_name

    if job.target_os == OS.WINDOWS:
        write_script_ps1(outside_script_base, before_script, script, after_script,
                         [
                             f"git config --global --add safe.directory \"{inside_dir}\"",
                             f"git config --global --add safe.directory \"{inside_git_dir}\"",
                             f". \"{inside_script_base}_before_script.ps1\"",
                             f". \"{inside_script_base}_script.ps1\"",
                             f". \"{inside_script_base}_after_script.ps1\""
                         ])
        return ["pwsh", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                "-Command", f"{inside_script_base}.ps1"]

    eval_string = ""
    for key, value in variables.items():
        eval_string += (f"eval {key}=\"{shquote(str(value))}\"\n"
                        f"printf %s=%s\\\\n {shquote(str(key))} {shquote(str(value))}\n")
    write_script_sh(outside_script_base, before_script, script, after_script, [
        eval_string,
        f"git config --global --add safe.directory {shquote(str(inside_dir))} || true",
        f"git config --global --add safe.directory {shquote(str(inside_git_dir))} || true",
        f". {shquote(f'{inside_script_base}_before_script.sh')}",
        f". {shquote(f'{inside_script_base}_script.sh')}",
        f". {shquote(f'{inside_script_base}_after_script.sh')}"
    ])

    ret = [
        "sh", "-xec",
        f"cd {shquote(str(inside_dir))}; "
        "if type bash > /dev/null 2>&1; then "
        f"bash {shquote(str(inside_script_base) + '.sh')}; "
        "else "
        f"sh {shquote(str(inside_script_base) + '.sh')}; "
        "fi"
    ]
    return ret


def clear_build(folder: Path, ret: int = 1) -> int:
    """Clears the build folder and removes the working directory"""
    shutil.rmtree(str(folder), ignore_errors=True)
    return ret


async def docker(project_dir: Path, variables: dict, image: str, extra_args: List[str],
                 log: IOBase) -> Process:
    """Runs docker for a job"""
    inside_path: Path = Path(
        "C:/" if HOST_OS == OS.WINDOWS else "/") / REPO_DIR.name
    args = ["run", "--rm"]
    if HOST_OS == OS.LINUX:
        args.extend(["--cap-add=SYS_PTRACE",
                    "--security-opt", "seccomp=unconfined"])
    args.extend(["--workdir", f"{inside_path}", "-v",
                 f"{project_dir}:{inside_path}",
                 "-e", "HTTP_PROXY", "-e", "HTTPS_PROXY",
                 "-e", f"CI_PROJECT_DIR={inside_path}"])
    for key, value in variables.items():
        args.append("-e")
        args.append(f"{key}={value}")
    if image:
        args.append(f"{DOCKER_REPO}/{image}")
    args.extend(extra_args)
    return await run_subprocess("docker", *args, log=log)


def pack_artifacts(project_dir: Path, artifacts: List[str]):
    """Packs the artifacts for a job"""
    if len(artifacts) == 0:
        return
    with tarfile.open(ARTIFACT_DIR / f"{project_dir.name}.tar.gz", "w:gz") as tar_file:
        for artifact in artifacts:
            for path in project_dir.glob(artifact):
                tar_file.add(path, path.relative_to(project_dir))


def extract_deps(deps: Set[str], project_dir: Path) -> int:
    """Extracts the dependencies for a job"""
    for dep in deps:
        dep_safe_name = urlquote(dep, safe='')
        tar_location = ARTIFACT_DIR / f"{dep_safe_name}.tar.gz"
        if not tar_location.exists():
            print(f"Error!: {dep} artifacts not found")
            return 1
        with tarfile.open(tar_location, "r:gz") as tar:
            # Added in 3.12
            if hasattr(tarfile, "data_filter"):
                tar.extractall(path=project_dir, filter="data")
            else:
                tar.extractall(path=project_dir)
    return 0


async def export_git_diff(project_dir: Path, log: IOBase):
    """export the current changes to the clone"""
    # test if there are any changes
    if await (await run_subprocess("git", "-C", REPO_DIR, "diff", "--quiet", log=log)).wait() == 0:
        return 0
    diff = await run_subprocess("git", "-C", REPO_DIR, "diff", log=log,  stdout=PIPE, stderr=log)
    apply = await run_subprocess("git", "-C", project_dir, "apply", "-3", log=log, stdin=PIPE)
    await apply.communicate(input=(await diff.communicate())[0])
    return await apply.wait()


async def run_job_epilog(job: Job, project_dir: Path, safe_name: str, ret: int = 0):
    """Runs the epilog for a job"""
    test_name = job["name"]
    running_log_file = RUNNING_LOG_DIR / f"{safe_name}.log"
    failed_log_file = FAILED_LOG_DIR / f"{safe_name}.log"
    finished_log_file = FINISHED_LOG_DIR / f"{safe_name}.log"
    print_status()
    with running_log_file.open("a") as log:
        print(f"Running epilog for {test_name}, ret {ret}", file=log)
    if HOST_OS == OS.LINUX:
        # if we're on linux, we need to chown the files back to the user
        # because docker runs as root, generally.
        inside_dir = Path("/") / REPO_DIR.name
        with running_log_file.open("a") as log:
            dock = await docker(project_dir, {}, "",
                                [DOCKER_HELPER_IMAGE,
                                 "sh", "-xc",
                                 f"chown -R {getuid()}:{getgid()} {shquote(str(inside_dir))}"],
                                log=log)
        await dock.wait()
    if ret != 0:
        # if it failed, move the log to the failed log folder
        # but leave the build directory intact for debugging
        running_log_file.replace(failed_log_file)
        return ret
    # if it succeeded, move the log to the finished log folder
    # and remove the build directory
    job.event.set()
    running_log_file.replace(finished_log_file)
    pack_artifacts(project_dir, job.artifacts)
    return clear_build(project_dir, ret)


async def run_job(test_name: str):
    """Runs a single job"""
    job: dict = JOBS[test_name]
    safe_name: str = urlquote(test_name, safe='')
    project_dir = BASE_DIR / "builds" / safe_name
    project_dir.parent.mkdir(parents=True, exist_ok=True)
    clear_build(project_dir)

    running_log_file = RUNNING_LOG_DIR / f"{safe_name}.log"
    failed_log_file = FAILED_LOG_DIR / f"{safe_name}.log"
    finished_log_file = FINISHED_LOG_DIR / f"{safe_name}.log"
    for log_file in (running_log_file, failed_log_file, finished_log_file):
        log_file.unlink(missing_ok=True)

    with running_log_file.open("w", encoding="utf-8") as log:
        print(f"Running {test_name}", file=log, flush=True)
        # print(job, file=log, flush=True)
        # Create out out of tree clone
        worktree = await run_subprocess("git", "clone", "--single-branch", "-l",
                                        REPO_DIR, project_dir, log=log)
        if await worktree.wait():
            print(
                f"Error cloning {test_name} into {project_dir}", file=log, flush=True)
            return await run_job_epilog(job, project_dir, safe_name, ret=1)

        if 'Default' not in test_name:
            if await export_git_diff(project_dir, log):
                print(
                    f"Error exporting git diff for {test_name}", file=log, flush=True)
                return await run_job_epilog(job, project_dir, safe_name, ret=1)

        # pull the dependency artifacts
        if extract_deps(job["needs"].keys(), project_dir):
            print(
                f"Error extracting dependencies for {test_name}", file=log, flush=True)
            return await run_job_epilog(job, project_dir, safe_name, ret=1)

        proc = await docker(project_dir,
                            job["variables"],
                            job["image"],
                            write_script(job, safe_name,
                                         project_dir, job["variables"]),
                            log)
        ret = await proc.wait()
        print(f"Finished {test_name} with ret {ret}", file=log, flush=True)
    return await run_job_epilog(job, project_dir, safe_name, ret=ret)


def extract_proj_name_pipe_id(url: str) -> Tuple[str, int]:
    """Extracts the project name and pipeline id"""
    # e.g. "https://gitlab.com/AOMediaCodec/SVT-AV1/-/pipelines/744815437"
    # -> ("AOMediaCodec/SVT-AV1", 744815437)
    parts = url.split("/-/pipelines/")
    project_name = parts[0]
    if project_name.startswith("https://gitlab.com/"):
        project_name = project_name[len("https://gitlab.com/"):]

    pipeline_id = int(parts[1])
    return (project_name, pipeline_id)


def retrieve_jobs(project_name: str, pipeline_id: int, scope: str = "") -> List[str]:
    """Retrieves the names of all jobs in a pipeline"""
    url = f"{get_api_url(project_name)}/pipelines/{pipeline_id}/jobs?per_page=100"
    if scope:
        url += f"&scope={scope}"

    req = request.Request(url)
    if "CI_JOB_TOKEN" in environ:
        req.add_header('PRIVATE-TOKEN', environ["CI_JOB_TOKEN"])
    if "HTTP_PROXY" in environ:
        req.set_proxy(environ["HTTP_PROXY"], "http")
    if "HTTPS_PROXY" in environ:
        req.set_proxy(environ["HTTPS_PROXY"], "https")

    job_names = []
    next_page = "1"
    while next_page:
        req.full_url = f"{url}&page={next_page}"
        with request.urlopen(req) as response:
            jobs = json.load(response)
            job_names.extend([job["name"].strip() for job in jobs])
            next_page = response.info()["X-Next-Page"]
    return job_names


class DepState(Enum):
    """The state of a dependency"""
    RUNNING = 1
    FINISHED = 2
    FAILED = 3


def check_deps(job_name: str) -> DepState:
    """Checks if all dependencies of a job have finished"""
    needs = frozenset(JOBS[job_name]["needs"])
    if needs & FAILED:
        return DepState.FAILED
    if needs.issubset(FINISHED):
        return DepState.FINISHED
    return DepState.RUNNING


async def start_runner(job_queue: Queue, num: int):
    """Starts a single runner"""
    while True:
        job_name = await job_queue.get()
        QUEUED.remove(job_name)
        with SCRIPT_LOG.open("a") as log:
            print(f"Runner {num} got {job_name}", file=log, flush=True)
        with SCRIPT_LOG.open("a") as log:
            print(f"Runner {num} running {job_name}", file=log, flush=True)
        RUNNING.add(job_name)
        print_status()
        if await run_job(job_name):
            with SCRIPT_LOG.open("a") as log:
                print(f"Job {job_name} failed", file=log, flush=True)
            FAILED.add(job_name)
            failures = generate_dependentent_set(job_name)
            for failure in failures:
                FAILED.add(failure)
        FINISHED.add(job_name)
        RUNNING.remove(job_name)
        job_queue.task_done()


async def start_runners(job_queue: Queue, finished_flag: Event):
    """Starts all runners"""
    runners = []
    for i in range(MAX_RUNNERS):
        runner = create_task(start_runner(job_queue, i), name=f"runner-{i}")
        runners.append(runner)

    # wait a bit for the jobs to be submitted
    await sleep(1)

    time_start = monotonic()
    await finished_flag.wait()
    await job_queue.join()
    time_end = monotonic()

    runner: Task
    for runner in runners:
        runner.cancel()
    await asyncio.gather(*runners, return_exceptions=True)

    print_status()

    with SCRIPT_LOG.open("a") as log:
        print(f"Failed jobs: {FAILED}", file=log, flush=True)
        print(f"Finished in {time_end - time_start} seconds",
              file=log, flush=True)
    print(f"Script finished in {time_end - time_start} seconds")


async def submit_job(job_queue: Queue, job_name: str):
    """Submits a job to the queue, submits needs first"""
    if job_name in FINISHED | FAILED | QUEUED:
        return
    job: dict = JOBS[job_name]
    if not job["needs"]:
        # no deps, submit directly
        QUEUED.add(job_name)
        job_queue.put_nowait(job_name)
        print_status()
        return
    deps = frozenset(job["needs"].keys())
    dep_tasks: List[Task] = []
    # submit deps first
    for dep in deps:
        dep_tasks.append(create_task(submit_job(
            job_queue, dep), name=f"submit-{dep}"))
    await asyncio.gather(*dep_tasks, return_exceptions=True)
    # wait for deps to finish
    await asyncio.gather(*[dep.event.wait() for dep in job["needs"].values()])
    if deps & FAILED:
        return

    # submit this job
    job_queue.put_nowait(job_name)
    QUEUED.add(job_name)
    print_status()


async def run_specific_jobs(job_queue: Queue, finished_flag: Event, name_list: Set[str]) -> int:
    """Runs a list of jobs"""
    jobs = []
    for job in set(name_list):
        jobs.append(create_task(submit_job(
            job_queue, job), name=f"submit-{job}"))
    await asyncio.gather(*jobs, return_exceptions=True)
    finished_flag.set()

    for job in name_list:
        if job in FAILED:
            print(
                f"{job} failed, please check the failed_logs folder")
            return 1
    return 0


async def run_all_jobs(job_queue: Queue, finished_flag: Event,
                       project_name: str = DEFAULT_PROJECT_NAME, pipeline_id: int = 0,
                       scope: str = "failed") -> int:
    """Runs all jobs that are either in the pipeline or all known jobs"""
    if pipeline_id != 0:
        return await run_specific_jobs(job_queue, finished_flag,
                                       retrieve_jobs(project_name, pipeline_id, scope))

    # If no pipeline, run all known jobs
    return await run_specific_jobs(job_queue, finished_flag,
                                   (key for key, job in JOBS.items() if not job["dependents"]))


def get_hash() -> int:
    """Gets a hash of the current state of the pipeline"""
    return hash(frozenset(RUNNING) | frozenset(QUEUED) | frozenset(FAILED))


def format_set_print(in_set: set, num: int = 7, char_limit: int = 31) -> str:
    """Formats a set for printing, limits units to *char_limit* characters, capped at *num* units"""
    return ", ".join([f"{unit[:char_limit-3]}..." if len(unit) > char_limit
                      else unit for unit in list(in_set)[:num]]) + \
        (f", ... ({len(in_set) - num} more)" if len(in_set) > num else "")


def print_status():
    """Prints the status of the pipeline"""
    print(f"{datetime.now()}\n"
          f"Queued  [{len(QUEUED):02}]: {format_set_print(QUEUED)}\n"
          f"Running [{len(RUNNING):02}]: {format_set_print(RUNNING)}\n"
          f"Failed  [{len(FAILED):02}]: {format_set_print(FAILED)}\n"
          f"Finished[{len(FINISHED):02}]: {format_set_print(FINISHED, 5, 20)}\n")


async def create_submitter(job_queue: Queue, finished_flag: Event, project: str, pipeline: int,
                           jobs: Set[str]) -> Task:
    """Creates a setter for the jobs"""
    if project and pipeline != 0:
        submitter = create_task(run_all_jobs(job_queue,
                                             finished_flag, project, pipeline, "failed"))
    elif jobs:
        submitter = create_task(run_specific_jobs(
            job_queue, finished_flag, jobs))
    else:
        submitter = create_task(run_all_jobs(
            job_queue, finished_flag))
    return submitter


def stderrprint(*args, **kwargs):
    """Prints to stderr"""
    print(*args, file=stderr, **kwargs)


def retrieve_previous_jobs() -> set[str]:
    """
    Retrieves the previous jobs from the previous_jobs file
    """
    try:
        return {line for line in PREVIOUS_JOB.read_text().splitlines()}
    except FileNotFoundError:
        return set()


def display_menu(previous_jobs: set[str]):
    """Displays the menu"""
    stderrprint("Menu:")
    stderrprint("\t1. Run all Tests")
    stderrprint("\t2. Run Specific Test(s)")
    stderrprint("\t3. Run Failed Pipeline Tests")
    stderrprint("\t4. List all Tests")
    stderrprint("\t5. Cleanup offline_ci related files")
    stderrprint("\t6. Exit")
    if previous_jobs:
        stderrprint(
            f"\t7. Run recent tests: {format_set_print(previous_jobs)}")


def shunquote(string: str) -> str:
    """Reverses the effect of shlex.quote"""
    if string.find("'") != -1:
        try:
            return str(literal_eval(string))
        except SyntaxError:
            pass
    return string


def loose_search_in_dict(search: str, dictionary: dict[str, Job]) -> str:
    """Loose search in a dictionary"""
    for item in dictionary:
        if search.lower() in item.lower():
            return item
    return ""


def read_tests_from_user() -> Set[str]:
    """Reads test names from user input"""
    jobs = set()
    while True:
        try:
            user_input = input("Enter Test Name: ")
        except EOFError:
            break
        if user_input in ('done', ''):
            break
        user_input = shunquote(user_input)
        job_name = ""
        if user_input in JOBS:
            job_name = user_input
        elif (job_name := loose_search_in_dict(user_input, JOBS)) != "":
            pass
        else:
            stderrprint(
                f"Invalid test name '{user_input}' entered, please try again")
            continue
        jobs.add(job_name)
    return jobs


async def main():
    """Main function"""
    if not read_json():
        stderrprint("Invalid yaml")
        return
    user_input = ""
    pipeline = 0
    jobs = set()
    project = DEFAULT_PROJECT_NAME

    previous_jobs = retrieve_previous_jobs()

    while True:
        display_menu(previous_jobs)
        try:
            choice = int(input("\nEnter your selection: "))
        except EOFError:
            stderrprint("Exiting...")
            return 0
        except ValueError:
            stderrprint("Invalid selection, please try again")
            continue

        if choice == 1:
            break
        if choice == 2:
            stderrprint(
                "Enter test names, one per line. Enter 'done' or control+D when finished.")
            jobs.update(read_tests_from_user())
            if not jobs:
                stderrprint("No tests entered!")
                continue
            break
        if choice == 3:
            user_input = input("Enter Pipeline URL: ")
            try:
                project, pipeline = extract_proj_name_pipe_id(user_input)
            except (ValueError, IndexError):
                stderrprint("Invalid URL")
                continue
            break
        if choice == 4:
            stderrprint("Listing all jobs: \n")
            for job in JOBS:
                print(f"{shquote(str(job))}")
            return 0
        if choice == 5:
            stderrprint("Cleaning up...")
            shutil.rmtree(str(BASE_DIR), ignore_errors=True)
            return 0
        if choice == 6:
            stderrprint("Exiting...")
            return 0
        if choice == 7 and previous_jobs:
            jobs = previous_jobs
            break
        stderrprint("Invalid selection. Please try again.")

    for directory in (FAILED_LOG_DIR, RUNNING_LOG_DIR, FINISHED_LOG_DIR, ARTIFACT_DIR):
        directory.mkdir(parents=True, exist_ok=True)

    with open(PREVIOUS_JOB, "w") as file:
        file.write("\n".join(jobs))

    job_queue = Queue()
    finished_flag = Event()  # Event to signal that all jobs have been queued
    runners = create_task(start_runners(job_queue, finished_flag))

    if jobs:
        stderrprint(f"Queued {len(jobs)} jobs")
    submitter = create_task(create_submitter(
        job_queue, finished_flag, project, pipeline, jobs))

    await submitter
    await runners
    return 1 if FAILED else 0

if __name__ == "__main__":
    dependency_checks()
    exit(asyncio.run(main()))
