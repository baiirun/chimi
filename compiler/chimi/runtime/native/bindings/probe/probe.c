/* std/binding_probe: exercises the binding API (G20) from the compiler's own
   tests. Uses only chimi_binding.h, as every binding must. */
#define _POSIX_C_SOURCE 200809L
#include "chimi_binding.h"
#include "probe_layout.h"

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
  if (!probe_nested_add(left, right, &sum))
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

/* `Some((text, [text]))` for the Text the bytes spell: a value refused
   anywhere inside makes the whole result refused. */
ChimiBindingResult *chimi_probe_decode(ChimiBindingCall *call) {
  const unsigned char *bytes;
  size_t length;
  int64_t integer;
  if (!chimi_binding_bytes(call, 0, &bytes, &length))
    return chimi_binding_defect(call, "decode takes Bytes");
  if (chimi_binding_integer(call, 0, &integer))
    return chimi_binding_defect(call, "Bytes read as an Int");
  ChimiBindingValue *text = chimi_binding_make_text(call, (const char *)bytes, length);
  ChimiBindingValue *items[] = {text, chimi_binding_make_list(call, &text, 1)};
  return chimi_binding_success(call, chimi_binding_some(call, chimi_binding_make_tuple(call, items, 2)));
}

/* Fails (or, when the Bool is true, defects) with the bytes as its message,
   up to their first U+0000, as a C string ends there. */
ChimiBindingResult *chimi_probe_refuse(ChimiBindingCall *call) {
  const unsigned char *bytes;
  size_t length;
  int as_defect;
  if (!chimi_binding_bytes(call, 0, &bytes, &length) || !chimi_binding_boolean(call, 1, &as_defect))
    return chimi_binding_defect(call, "refuse takes Bytes and a Bool");
  char *message = malloc(length + 1);
  if (message == NULL) return chimi_binding_defect(call, "out of memory");
  memcpy(message, bytes, length);
  message[length] = 0;
  ChimiBindingResult *result =
    as_defect ? chimi_binding_defect(call, message) : chimi_binding_failure(call, PROBE_INVALID, message);
  free(message);
  return result;
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
      int waited = chimi_binding_wait(call, 0, state->write_end, CHIMI_BINDING_WRITABLE);
      if (waited == CHIMI_BINDING_WAIT_CANCELLED) return chimi_binding_cancelled(call);
      if (waited != CHIMI_BINDING_READY) break;
      state = chimi_binding_handle(call, 0, &PIPE);
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
    int waited = chimi_binding_wait(call, 0, state->read_end, CHIMI_BINDING_READABLE);
    /* Asks the scheduler once more on the way out, as a binding may, so a
       test can check that a forced body's last request lets its handle go. */
    if (waited == CHIMI_BINDING_WAIT_CANCELLED && chimi_binding_is_cancelled(call))
      return chimi_binding_cancelled(call);
    if (waited == CHIMI_BINDING_WAIT_CANCELLED) return chimi_binding_cancelled(call);
    if (waited != CHIMI_BINDING_READY)
      return chimi_binding_failure(call, PROBE_CLOSED, "pipe is closed");
  }
}

ChimiBindingResult *chimi_probe_close_pipe(ChimiBindingCall *call) {
  chimi_binding_close_handle(call, 0, &PIPE);
  return chimi_binding_success(call, chimi_binding_void(call));
}
