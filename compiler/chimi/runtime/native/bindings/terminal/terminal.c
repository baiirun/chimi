/* std/terminal: whether a standard stream is a terminal, and its size (G20).
   Both are one non-blocking system call on the stream's descriptor, so
   neither waits on the scheduler. A stream is its std/terminal.Standard
   code: 0 input, 1 output, 2 error output. */

#include "chimi_binding.h"

#include <stdint.h>
#include <sys/ioctl.h>
#include <unistd.h>

/* The descriptor for `code`, or -1 for a code outside 0..2. */
static int descriptor(const ChimiBindingCall *call) {
  int64_t code;
  if (!chimi_binding_integer(call, 0, &code) || code < 0 || code > 2) return -1;
  return (int)code;
}

/* native_is_terminal(stream) -> Bool */
ChimiBindingResult *chimi_terminal_is_terminal(ChimiBindingCall *call) {
  int fd = descriptor(call);
  if (fd < 0) return chimi_binding_defect(call, "Terminal: not a standard stream");
  return chimi_binding_success(call, chimi_binding_make_boolean(call, isatty(fd) == 1));
}

/* native_size(stream) -> Option<(Int, Int)>: columns and rows, or None when
   the stream is not a terminal or the terminal reports no size (0 by 0, as
   serial consoles do). */
ChimiBindingResult *chimi_terminal_size(ChimiBindingCall *call) {
  int fd = descriptor(call);
  if (fd < 0) return chimi_binding_defect(call, "Terminal: not a standard stream");
  struct winsize size;
  if (!isatty(fd) || ioctl(fd, TIOCGWINSZ, &size) != 0 || size.ws_col == 0 || size.ws_row == 0)
    return chimi_binding_success(call, chimi_binding_none(call));
  ChimiBindingValue *items[2] = {chimi_binding_make_integer(call, size.ws_col),
                                 chimi_binding_make_integer(call, size.ws_row)};
  return chimi_binding_success(call, chimi_binding_some(call, chimi_binding_make_tuple(call, items, 2)));
}
