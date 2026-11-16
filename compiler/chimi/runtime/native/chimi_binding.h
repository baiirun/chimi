/* The binding API: the only interface C code of a standard binding package
   (G20) may use. A binding function implements one `native fn`; the runtime
   calls it with the call's arguments and takes back exactly one result.

   standard-modules-and-constants.md, "Native bindings for std and vendored
   modules (G20)", is the contract. In short:

   - Values are borrowed for the duration of the call. Never keep a
     ChimiBindingValue, a text or bytes pointer, or the call after returning.
   - Build every result with the functions below, and return one of
     chimi_binding_success, chimi_binding_failure, chimi_binding_defect, or
     chimi_binding_cancelled.
   - An expected failure is the value `Err((code, message))` of the `native
     fn`'s declared `Result<T, (Int, Text)>` return type; the Chimi wrapper
     turns the code into its module's failure enum. Success of such a function
     is chimi_binding_ok.
   - Wait for a descriptor with chimi_binding_wait, never with a blocking
     call, so other tasks keep running. Make descriptors non-blocking.
   - Never close a descriptor while another task waits on it: the reactor
     waits on the descriptor's number, which the next open may reuse. A
     handle whose descriptor tasks wait on must stop them first (the runtime
     does not yet offer a way to wake them; see the spec's "Not covered
     yet").
   - chimi_binding_wait lets other tasks run, and one may close a handle
     argument meanwhile: pass the handle's index so the wait answers
     CHIMI_BINDING_WAIT_CLOSED, and read its state again with
     chimi_binding_handle after every wait.
   - A host resource is a handle: chimi_binding_make_handle gives the runtime
     ownership of `state`, and the type's release runs exactly once, at
     chimi_binding_close_handle or when the program ends. Chimi code never sees
     the pointer.

   Calls run on the scheduler thread, one at a time; the API is not
   thread-safe. */
#ifndef CHIMI_BINDING_H
#define CHIMI_BINDING_H

#include <stddef.h>
#include <stdint.h>

#define CHIMI_BINDING_ABI_VERSION 1

typedef struct ChimiBindingCall ChimiBindingCall;
typedef struct ChimiBindingValue ChimiBindingValue;
typedef struct ChimiBindingResult ChimiBindingResult;
typedef ChimiBindingResult *(*ChimiBindingFunction)(ChimiBindingCall *call);

/* The kind of resource behind a handle. `name` is what Debug.render shows,
   `<name>`. `release` frees `state`; it must not fail or call the API. */
typedef struct {
  const char *name;
  void (*release)(void *state);
} ChimiBindingType;

/* Arguments. Each reader checks the argument's runtime kind and returns false
   (or NULL) on a mismatch, which a verified program never produces; return
   chimi_binding_defect then. */
uint32_t chimi_binding_argument_count(const ChimiBindingCall *call);
int chimi_binding_integer(const ChimiBindingCall *call, uint32_t index, int64_t *out);
int chimi_binding_boolean(const ChimiBindingCall *call, uint32_t index, int *out);
/* Text is UTF-8 and NUL-terminated, and may also hold U+0000 before `length`. */
int chimi_binding_text(const ChimiBindingCall *call, uint32_t index, const char **bytes,
                       size_t *length);
int chimi_binding_bytes(const ChimiBindingCall *call, uint32_t index,
                        const unsigned char **bytes, size_t *length);
/* The state of an open handle of `type`; NULL when the argument is closed. */
void *chimi_binding_handle(const ChimiBindingCall *call, uint32_t index,
                           const ChimiBindingType *type);

/* Values. Text must be valid UTF-8; invalid text is a defect at return. */
ChimiBindingValue *chimi_binding_void(ChimiBindingCall *call);
ChimiBindingValue *chimi_binding_make_integer(ChimiBindingCall *call, int64_t value);
ChimiBindingValue *chimi_binding_make_boolean(ChimiBindingCall *call, int value);
ChimiBindingValue *chimi_binding_make_text(ChimiBindingCall *call, const char *bytes,
                                           size_t length);
ChimiBindingValue *chimi_binding_make_bytes(ChimiBindingCall *call, const unsigned char *bytes,
                                            size_t length);
ChimiBindingValue *chimi_binding_none(ChimiBindingCall *call);
ChimiBindingValue *chimi_binding_some(ChimiBindingCall *call, ChimiBindingValue *value);
ChimiBindingValue *chimi_binding_make_tuple(ChimiBindingCall *call, ChimiBindingValue **items,
                                            uint32_t length);
ChimiBindingValue *chimi_binding_make_list(ChimiBindingCall *call, ChimiBindingValue **items,
                                           uint32_t length);
/* A new handle that owns `state`. NULL when the handle table is full, after
   releasing `state`. */
ChimiBindingValue *chimi_binding_make_handle(ChimiBindingCall *call, const ChimiBindingType *type,
                                             void *state);
/* Releases the handle argument at `index` if it is an open handle of `type`,
   so later calls see it closed; closing a closed handle does nothing. */
void chimi_binding_close_handle(ChimiBindingCall *call, uint32_t index,
                                const ChimiBindingType *type);

/* Results: return exactly one. */
ChimiBindingResult *chimi_binding_success(ChimiBindingCall *call, ChimiBindingValue *value);
/* `Ok(value)`, for a `native fn` that returns `Result<T, (Int, Text)>`. */
ChimiBindingResult *chimi_binding_ok(ChimiBindingCall *call, ChimiBindingValue *value);
/* `Err((code, message))`; `message` is UTF-8 and NUL-terminated. */
ChimiBindingResult *chimi_binding_failure(ChimiBindingCall *call, int64_t code,
                                          const char *message);
ChimiBindingResult *chimi_binding_defect(ChimiBindingCall *call, const char *message);
ChimiBindingResult *chimi_binding_cancelled(ChimiBindingCall *call);

/* Scheduling. */
int chimi_binding_is_cancelled(const ChimiBindingCall *call);
enum {
  CHIMI_BINDING_READABLE = 1,
  CHIMI_BINDING_WRITABLE = 2,
};
enum {
  /* The descriptor reported the interest, or a hangup or error that the next
     read or write reports. */
  CHIMI_BINDING_READY = 0,
  /* The task was cancelled while it waited: return chimi_binding_cancelled. */
  CHIMI_BINDING_WAIT_CANCELLED = 1,
  /* The descriptor cannot be waited on (closed, negative, or the reactor
     failed). */
  CHIMI_BINDING_WAIT_FAILED = 2,
  /* The handle argument the descriptor belongs to was closed, before or
     during the wait: report the resource closed, and do not touch `fd`. */
  CHIMI_BINDING_WAIT_CLOSED = 3,
};
/* `handle` for a descriptor that belongs to no handle argument. */
#define CHIMI_BINDING_NO_HANDLE UINT32_MAX
/* Waits until `fd`, which belongs to the handle argument at index `handle`
   (or CHIMI_BINDING_NO_HANDLE for a descriptor private to the call), is ready
   for `interest`, letting other tasks run. */
int chimi_binding_wait(ChimiBindingCall *call, uint32_t handle, int fd, int interest);

#endif
