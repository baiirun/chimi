/* A header in a subdirectory, found through the package's own -Iinclude flag
   (BINDING_PACKAGE_FLAGS), as a vendored library's headers are. */
#ifndef PROBE_LAYOUT_H
#define PROBE_LAYOUT_H

#include <stdint.h>

#ifndef PROBE_PACKAGE_FLAGS
#error "the probe package's flags were not passed to the C compiler"
#endif

/* Defined in nested/add.c, a source in a subdirectory. Returns 0 on overflow. */
int probe_nested_add(int64_t left, int64_t right, int64_t *sum);

#endif
