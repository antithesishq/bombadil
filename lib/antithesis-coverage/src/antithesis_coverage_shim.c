// antithesis_coverage_shim.c
#include <dlfcn.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>

typedef void (*init_coverage_module_fn)(size_t edge_count,
                                        const char *symbol_file_name);
typedef void (*notify_coverage_fn)(
    size_t edge_index); // CONFIRM against nm -D output

static void *voidstar_handle = NULL;
static init_coverage_module_fn real_init = NULL;
static notify_coverage_fn real_notify = NULL;
static bool did_check = false;
static bool has_libvoidstar = false;

static void debug_out(const char *msg) {
  if (getenv("ANTITHESIS_OUTPUT_DIR") != NULL)
    fprintf(stderr, "%s\n", msg);
}

static void ensure_loaded(void) {
  if (did_check)
    return;
  did_check = true;
  voidstar_handle = dlopen("/usr/lib/libvoidstar.so", RTLD_NOW);
  if (!voidstar_handle) {
    char msg[512];
    snprintf(msg, sizeof(msg), "libvoidstar dlopen failed: %s", dlerror());
    debug_out(msg);
    return;
  }
  if (!voidstar_handle) {
    debug_out("libvoidstar not available; coverage calls will no-op");
    return;
  }
  real_init =
      (init_coverage_module_fn)dlsym(voidstar_handle, "init_coverage_module");
  real_notify = (notify_coverage_fn)dlsym(voidstar_handle, "notify_coverage");
  if (!real_init || !real_notify) {
    debug_out(
        "libvoidstar missing expected symbols; coverage calls will no-op");
    real_init = NULL;
    real_notify = NULL;
    return;
  }
  has_libvoidstar = true;
  debug_out("LOADED libvoidstar");
}

// Called eagerly via .init_array from Rust
void antithesis_load_libvoidstar(void) { ensure_loaded(); }

void antithesis_init_coverage_module(size_t edge_count,
                                     const char *symbol_file_name) {
  ensure_loaded();
  if (has_libvoidstar)
    real_init(edge_count, symbol_file_name);
}

void antithesis_notify_coverage(size_t edge_index) {
  if (has_libvoidstar)
    real_notify(edge_index);
}
