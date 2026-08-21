#include <dlfcn.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>

static void *libvoidstar_handle = NULL;
static bool libvoidstar_checked = false;
static bool libvoidstar_loaded = false;

typedef void (*init_coverage_module_fn)(size_t edge_count,
                                        const char *symbol_file_name);
typedef void (*notify_coverage_fn)(size_t edge_index);
typedef int (*fuzz_getchar_fn)();

static init_coverage_module_fn init_coverage_module_real = NULL;
static notify_coverage_fn notify_coverage_real = NULL;
static fuzz_getchar_fn fuzz_getchar_real = NULL;

static void debug_out(const char *msg) {
  if (getenv("ANTITHESIS_OUTPUT_DIR") != NULL) {
    fprintf(stderr, "%s\n", msg);
  }
}

static void libvoidstar_ensure_loaded(void) {
  if (libvoidstar_checked) {
    return;
  }
  libvoidstar_checked = true;
  libvoidstar_handle = dlopen("/usr/lib/libvoidstar.so", RTLD_NOW);
  if (!libvoidstar_handle) {
    char msg[512];
    snprintf(msg, sizeof(msg), "libvoidstar dlopen failed: %s", dlerror());
    debug_out(msg);
    return;
  }
  if (!libvoidstar_handle) {
    debug_out("libvoidstar not available; coverage calls will no-op");
    return;
  }
  init_coverage_module_real = (init_coverage_module_fn)dlsym(
      libvoidstar_handle, "init_coverage_module");
  notify_coverage_real =
      (notify_coverage_fn)dlsym(libvoidstar_handle, "notify_coverage");
  fuzz_getchar_real =
      (fuzz_getchar_fn)dlsym(libvoidstar_handle, "fuzz_getchar");
  if (!init_coverage_module_real || !notify_coverage_real ||
      !fuzz_getchar_real) {
    debug_out(
        "libvoidstar missing expected symbols; coverage calls will no-op");
    init_coverage_module_real = NULL;
    notify_coverage_real = NULL;
    return;
  }
  libvoidstar_loaded = true;
  debug_out("libvoidstar dlopen successful!");
}

// This is called from the init section.
void antithesis_load_libvoidstar(void) { libvoidstar_ensure_loaded(); }

void antithesis_init_coverage_module(size_t edge_count,
                                     const char *symbol_file_name) {
  libvoidstar_ensure_loaded();
  if (libvoidstar_loaded) {
    init_coverage_module_real(edge_count, symbol_file_name);
  }
}

void antithesis_notify_coverage(size_t edge_index) {
  if (libvoidstar_loaded) {
    notify_coverage_real(edge_index);
  }
}

int antithesis_fuzz_getchar() {
  if (libvoidstar_loaded) {
    return fuzz_getchar_real();
  } else {
    return 0;
  }
}
