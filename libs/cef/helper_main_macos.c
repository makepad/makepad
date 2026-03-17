#include <dlfcn.h>
#include <libgen.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "capi/cef_app_capi.h"
#include "cef_api_hash.h"

#ifndef RTLD_FIRST
#define RTLD_FIRST 0x100
#endif

typedef const char* (*cef_api_hash_fn)(int version, int entry);
typedef int (*cef_execute_process_fn)(const cef_main_args_t* args,
                                      cef_app_t* application,
                                      void* windows_sandbox_info);

static int get_framework_path(char* buffer, size_t buffer_size) {
  uint32_t exec_path_size = (uint32_t)buffer_size;
  if (_NSGetExecutablePath(buffer, &exec_path_size) != 0) {
    return 0;
  }

  char exec_dir[PATH_MAX];
  strncpy(exec_dir, buffer, sizeof(exec_dir) - 1);
  exec_dir[sizeof(exec_dir) - 1] = '\0';

  char* parent_dir = dirname(exec_dir);
  if (!parent_dir) {
    return 0;
  }

  int written = snprintf(
      buffer,
      buffer_size,
      "%s/../../../Chromium Embedded Framework.framework/Chromium Embedded Framework",
      parent_dir);
  return written > 0 && (size_t)written < buffer_size;
}

int main(int argc, char* argv[]) {
  char framework_path[PATH_MAX];
  if (!get_framework_path(framework_path, sizeof(framework_path))) {
    fprintf(stderr, "makepad-cef-helper: failed to resolve framework path\n");
    return 1;
  }

  void* handle = dlopen(framework_path, RTLD_LAZY | RTLD_LOCAL | RTLD_FIRST);
  if (!handle) {
    fprintf(stderr, "makepad-cef-helper: dlopen %s failed: %s\n", framework_path, dlerror());
    return 1;
  }

  cef_api_hash_fn cef_api_hash_ptr =
      (cef_api_hash_fn)dlsym(handle, "cef_api_hash");
  cef_execute_process_fn cef_execute_process_ptr =
      (cef_execute_process_fn)dlsym(handle, "cef_execute_process");

  if (!cef_api_hash_ptr || !cef_execute_process_ptr) {
    fprintf(stderr, "makepad-cef-helper: failed to resolve required CEF symbols\n");
    dlclose(handle);
    return 1;
  }

  if (!cef_api_hash_ptr(CEF_API_VERSION, 0)) {
    fprintf(stderr, "makepad-cef-helper: cef_api_hash returned null\n");
    dlclose(handle);
    return 1;
  }

  cef_main_args_t main_args = {};
  main_args.argc = argc;
  main_args.argv = argv;

  int result = cef_execute_process_ptr(&main_args, NULL, NULL);
  dlclose(handle);
  return result;
}
