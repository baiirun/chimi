/* std/binding_probe: exercises the binding API (G20) from the compiler's own
   tests. Uses only chimi_binding.h, as every binding must. */
#define _POSIX_C_SOURCE 200809L
#include "chimi_binding.h"

#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

enum { PROBE_INVALID = 1, PROBE_CLOSED = 2 };

ChimiBindingResult *chimi_probe_echo(ChimiBindingCall *call) {
  const unsigned char *bytes;
  size_t length;
  if (!chimi_binding_bytes(call, 0, &bytes, &length))
    return chimi_binding_defect(call, "echo takes Bytes");
  return chimi_binding_success(call, chimi_binding_make_bytes(call, bytes, length));
}

ChimiBindingResult *chimi_probe_add(ChimiBindingCall *call) {
  int64_t left, right, sum;
  if (!chimi_binding_integer(call, 0, &left) || !chimi_binding_integer(call, 1, &right))
    return chimi_binding_defect(call, "add takes two Ints");
  if (__builtin_add_overflow(left, right, &sum))
    return chimi_binding_defect(call, "Integer overflow");
  return chimi_binding_success(call, chimi_binding_make_integer(call, sum));
}

ChimiBindingResult *chimi_probe_try_parse(ChimiBindingCall *call) {
  const char *text;
  size_t length;
  if (!chimi_binding_text(call, 0, &text, &length))
    return chimi_binding_defect(call, "try_parse takes Text");
  size_t index = text[0] == '-' ? 1 : 0;
  if (index == length) return chimi_binding_failure(call, PROBE_INVALID, "no digits");
  int64_t value = 0;
  for (; index < length; ++index) {
    if (text[index] < '0' || text[index] > '9')
      return chimi_binding_failure(call, PROBE_INVALID, "not a decimal digit");
    int64_t digit = text[index] - '0';
    if (__builtin_mul_overflow(value, 10, &value) ||
        __builtin_sub_overflow(value, digit, &value))
      return chimi_binding_failure(call, PROBE_INVALID, "out of range");
  }
  if (text[0] != '-' && __builtin_sub_overflow((int64_t)0, value, &value))
    return chimi_binding_failure(call, PROBE_INVALID, "out of range");
  return chimi_binding_ok(call, chimi_binding_make_integer(call, value));
}

ChimiBindingResult *chimi_probe_crash(ChimiBindingCall *call) {
  const char *message;
  size_t length;
  if (!chimi_binding_text(call, 0, &message, &length))
    return chimi_binding_defect(call, "crash takes Text");
  return chimi_binding_defect(call, message);
}

typedef struct {
  int read_end;
  int write_end;
} Pipe;

static void release_pipe(void *state) {
  Pipe *pipe_ends = state;
  close(pipe_ends->read_end);
  close(pipe_ends->write_end);
  free(pipe_ends);
}

static const ChimiBindingType PIPE = {"Pipe", release_pipe};

ChimiBindingResult *chimi_probe_open_pipe(ChimiBindingCall *call) {
  int ends[2];
  if (pipe(ends) != 0) return chimi_binding_defect(call, strerror(errno));
  for (int index = 0; index < 2; ++index) {
    int flags = fcntl(ends[index], F_GETFL);
    fcntl(ends[index], F_SETFL, flags | O_NONBLOCK);
    fcntl(ends[index], F_SETFD, FD_CLOEXEC);
  }
  Pipe *state = malloc(sizeof(*state));
  if (state == NULL) {
    close(ends[0]);
    close(ends[1]);
    return chimi_binding_defect(call, "out of memory");
  }
  state->read_end = ends[0];
  state->write_end = ends[1];
  ChimiBindingValue *handle = chimi_binding_make_handle(call, &PIPE, state);
  if (handle == NULL) return chimi_binding_defect(call, "too many open handles");
  return chimi_binding_success(call, handle);
}

ChimiBindingResult *chimi_probe_write_pipe(ChimiBindingCall *call) {
  Pipe *state = chimi_binding_handle(call, 0, &PIPE);
  const unsigned char *bytes;
  size_t length;
  if (!chimi_binding_bytes(call, 1, &bytes, &length))
    return chimi_binding_defect(call, "write_pipe takes Bytes");
  if (state == NULL) return chimi_binding_failure(call, PROBE_CLOSED, "pipe is closed");
  size_t written = 0;
  while (written < length) {
    ssize_t count = write(state->write_end, bytes + written, length - written);
    if (count > 0) {
      written += (size_t)count;
      continue;
    }
    if (count < 0 && errno == EINTR) continue;
    if (count < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
      int waited = chimi_binding_wait(call, state->write_end, CHIMI_BINDING_WRITABLE);
      if (waited == CHIMI_BINDING_WAIT_CANCELLED) return chimi_binding_cancelled(call);
      if (waited == CHIMI_BINDING_WAIT_FAILED) break;
      /* The handle may have been closed by another task while this one waited. */
      state = chimi_binding_handle(call, 0, &PIPE);
      if (state == NULL) break;
      continue;
    }
    break;
  }
  if (written < length) return chimi_binding_failure(call, PROBE_CLOSED, "pipe is closed");
  return chimi_binding_ok(call, chimi_binding_void(call));
}

ChimiBindingResult *chimi_probe_read_pipe(ChimiBindingCall *call) {
  unsigned char buffer[64 * 1024];
  for (;;) {
    Pipe *state = chimi_binding_handle(call, 0, &PIPE);
    if (state == NULL) return chimi_binding_failure(call, PROBE_CLOSED, "pipe is closed");
    ssize_t count = read(state->read_end, buffer, sizeof(buffer));
    if (count > 0)
      return chimi_binding_ok(
        call, chimi_binding_some(call, chimi_binding_make_bytes(call, buffer, (size_t)count)));
    if (count == 0) return chimi_binding_ok(call, chimi_binding_none(call));
    if (errno == EINTR) continue;
    if (errno != EAGAIN && errno != EWOULDBLOCK)
      return chimi_binding_failure(call, PROBE_CLOSED, strerror(errno));
    int waited = chimi_binding_wait(call, state->read_end, CHIMI_BINDING_READABLE);
    if (waited == CHIMI_BINDING_WAIT_CANCELLED) return chimi_binding_cancelled(call);
    if (waited == CHIMI_BINDING_WAIT_FAILED)
      return chimi_binding_failure(call, PROBE_CLOSED, "pipe is closed");
  }
}

ChimiBindingResult *chimi_probe_close_pipe(ChimiBindingCall *call) {
  chimi_binding_close_handle(call, 0);
  return chimi_binding_success(call, chimi_binding_void(call));
}
