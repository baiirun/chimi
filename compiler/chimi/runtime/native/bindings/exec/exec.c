/* std/exec: child processes over the binding API (G20). A command is built
   on a Builder handle one piece at a time, then started with posix_spawn.
   The program is looked up on the parent's PATH here, before the spawn, so a
   missing or non-executable program fails at once with its own code. The
   child gets the default action and an empty mask for every signal the
   runtime handles, so it reacts to them as if a shell had started it.

   Waiting goes through chimi_binding_wait_child, which leaves the child
   unreaped; this file is the only one that reaps it, by pid, and records
   its status in the Child handle, so a second wait answers from the record
   and a signal is never sent to a pid after it was reaped (and possibly
   reused). Releasing a Child that is still running kills and reaps it. */
#define _POSIX_C_SOURCE 200809L
#define _DEFAULT_SOURCE
#define _DARWIN_C_SOURCE
#define _GNU_SOURCE
#include "chimi_binding.h"

#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <spawn.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

/* The failure codes the Chimi wrapper maps to `ExecError`. */
enum {
  EXEC_NOT_FOUND = 1,
  EXEC_PERMISSION_DENIED = 2,
  EXEC_INVALID_DIRECTORY = 3,
  EXEC_CLOSED = 4,
  EXEC_FAILED = 5,
};

/* How a standard stream of the child is connected (std/exec `Stdio`). */
enum { STDIO_INHERIT = 0, STDIO_NULL = 1 };

typedef struct {
  char **items; /* NULL-terminated */
  size_t count;
  size_t capacity;
} TextList;

typedef struct {
  char *program;
  TextList arguments;  /* argv[1..] */
  TextList variables;  /* "NAME=value", later entries win */
  bool clear_environment;
  char *directory;     /* NULL: the parent's */
} Builder;

typedef struct {
  pid_t pid;
  bool reaped;
  int status; /* waitpid's, once reaped */
} ChildProcess;

static void free_list(TextList *list) {
  for (size_t index = 0; index < list->count; ++index) free(list->items[index]);
  free(list->items);
}

static void release_builder(void *state) {
  Builder *builder = state;
  free(builder->program);
  free_list(&builder->arguments);
  free_list(&builder->variables);
  free(builder->directory);
  free(builder);
}

/* A Child still running when its handle is released (its scope ended, or
   the program did) is killed and reaped, so no child outlives its handle
   and no zombie stays behind. */
static void release_child(void *state) {
  ChildProcess *child = state;
  if (!child->reaped) {
    kill(child->pid, SIGKILL);
    int status;
    while (waitpid(child->pid, &status, 0) < 0 && errno == EINTR) {
    }
  }
  free(child);
}

static const ChimiBindingType BUILDER = {"Command", release_builder};
static const ChimiBindingType CHILD = {"Child", release_child};

static bool push(TextList *list, const char *text) {
  if (list->count + 2 > list->capacity) {
    size_t capacity = list->capacity ? list->capacity * 2 : 8;
    char **items = realloc(list->items, capacity * sizeof(char *));
    if (items == NULL) return false;
    list->items = items;
    list->capacity = capacity;
  }
  char *copy = strdup(text);
  if (copy == NULL) return false;
  list->items[list->count++] = copy;
  list->items[list->count] = NULL;
  return true;
}

/* Text without U+0000: C sees text only up to its first NUL, so a program,
   argument or variable holding one would silently be cut short. */
static bool text_argument(ChimiBindingCall *call, uint32_t index, const char **text) {
  size_t length;
  return chimi_binding_text(call, index, text, &length) && strlen(*text) == length;
}

ChimiBindingResult *chimi_exec_command(ChimiBindingCall *call) {
  const char *program;
  if (!text_argument(call, 0, &program)) return chimi_binding_defect(call, "Exec: a program name holds U+0000");
  Builder *builder = calloc(1, sizeof(Builder));
  if (builder == NULL || (builder->program = strdup(program)) == NULL) {
    free(builder);
    return chimi_binding_defect(call, "Exec: out of memory");
  }
  ChimiBindingValue *handle = chimi_binding_make_handle(call, &BUILDER, builder);
  if (handle == NULL) return chimi_binding_defect(call, "Exec: too many commands at once");
  return chimi_binding_success(call, handle);
}

ChimiBindingResult *chimi_exec_argument(ChimiBindingCall *call) {
  Builder *builder = chimi_binding_handle(call, 0, &BUILDER);
  const char *argument;
  if (builder == NULL) return chimi_binding_defect(call, "Exec: the command was already started");
  if (!text_argument(call, 1, &argument)) return chimi_binding_defect(call, "Exec: an argument holds U+0000");
  if (!push(&builder->arguments, argument)) return chimi_binding_defect(call, "Exec: out of memory");
  return chimi_binding_success(call, chimi_binding_void(call));
}

ChimiBindingResult *chimi_exec_variable(ChimiBindingCall *call) {
  Builder *builder = chimi_binding_handle(call, 0, &BUILDER);
  const char *name, *value;
  if (builder == NULL) return chimi_binding_defect(call, "Exec: the command was already started");
  if (!text_argument(call, 1, &name) || !text_argument(call, 2, &value))
    return chimi_binding_defect(call, "Exec: an environment variable holds U+0000");
  if (name[0] == '\0' || strchr(name, '=') != NULL)
    return chimi_binding_defect(call, "Exec: an environment variable name is empty or holds '='");
  size_t length = strlen(name) + strlen(value) + 2;
  char *entry = malloc(length);
  if (entry == NULL) return chimi_binding_defect(call, "Exec: out of memory");
  snprintf(entry, length, "%s=%s", name, value);
  bool pushed = push(&builder->variables, entry);
  free(entry);
  if (!pushed) return chimi_binding_defect(call, "Exec: out of memory");
  return chimi_binding_success(call, chimi_binding_void(call));
}

ChimiBindingResult *chimi_exec_clear_environment(ChimiBindingCall *call) {
  Builder *builder = chimi_binding_handle(call, 0, &BUILDER);
  if (builder == NULL) return chimi_binding_defect(call, "Exec: the command was already started");
  builder->clear_environment = true;
  return chimi_binding_success(call, chimi_binding_void(call));
}

ChimiBindingResult *chimi_exec_directory(ChimiBindingCall *call) {
  Builder *builder = chimi_binding_handle(call, 0, &BUILDER);
  const char *directory;
  if (builder == NULL) return chimi_binding_defect(call, "Exec: the command was already started");
  if (!text_argument(call, 1, &directory)) return chimi_binding_defect(call, "Exec: a directory holds U+0000");
  free(builder->directory);
  if ((builder->directory = strdup(directory)) == NULL) return chimi_binding_defect(call, "Exec: out of memory");
  return chimi_binding_success(call, chimi_binding_void(call));
}

/* An executable regular file. */
static bool executable(const char *path, int *error) {
  struct stat info;
  if (stat(path, &info) != 0) {
    *error = errno;
    return false;
  }
  if (S_ISDIR(info.st_mode) || access(path, X_OK) != 0) {
    *error = EACCES;
    return false;
  }
  return true;
}

/* The path to spawn for `program`, looked up on the parent's PATH when it
   names no directory, as a shell does; NULL with `code` set otherwise. A
   program found only without execute permission is PermissionDenied. */
static char *resolve(const char *program, int *code) {
  int error = ENOENT;
  if (program[0] == '\0') {
    *code = EXEC_NOT_FOUND;
    return NULL;
  }
  if (strchr(program, '/') != NULL) {
    if (executable(program, &error)) return strdup(program);
    *code = error == EACCES ? EXEC_PERMISSION_DENIED : EXEC_NOT_FOUND;
    return NULL;
  }
  const char *path = getenv("PATH");
  if (path == NULL || path[0] == '\0') path = "/usr/bin:/bin";
  bool denied = false;
  size_t program_length = strlen(program);
  for (const char *start = path;; ) {
    const char *end = strchr(start, ':');
    size_t length = end ? (size_t)(end - start) : strlen(start);
    /* An empty entry is the current directory. */
    const char *directory = length ? start : ".";
    size_t directory_length = length ? length : 1;
    char *candidate = malloc(directory_length + program_length + 2);
    if (candidate == NULL) {
      *code = EXEC_FAILED;
      return NULL;
    }
    memcpy(candidate, directory, directory_length);
    candidate[directory_length] = '/';
    memcpy(candidate + directory_length + 1, program, program_length + 1);
    if (executable(candidate, &error)) return candidate;
    denied = denied || error == EACCES;
    free(candidate);
    if (end == NULL) break;
    start = end + 1;
  }
  *code = denied ? EXEC_PERMISSION_DENIED : EXEC_NOT_FOUND;
  return NULL;
}

/* The child's environment: the parent's (unless cleared) with the
   command's variables set over it. Owned strings are the builder's or the
   parent's; only the array is new. */
static char **child_environment(const Builder *builder) {
  size_t inherited = 0;
  if (!builder->clear_environment)
    while (environ[inherited] != NULL) ++inherited;
  char **entries = calloc(inherited + builder->variables.count + 1, sizeof(char *));
  if (entries == NULL) return NULL;
  size_t count = 0;
  for (size_t index = 0; index < inherited; ++index) {
    const char *entry = environ[index];
    const char *equals = strchr(entry, '=');
    size_t name_length = equals ? (size_t)(equals - entry) : strlen(entry);
    bool replaced = false;
    for (size_t variable = 0; variable < builder->variables.count && !replaced; ++variable) {
      const char *set = builder->variables.items[variable];
      replaced = strncmp(set, entry, name_length) == 0 && set[name_length] == '=';
    }
    if (!replaced) entries[count++] = (char *)entry;
  }
  /* A later variable of the same name wins: keep only the last of each. */
  for (size_t variable = 0; variable < builder->variables.count; ++variable) {
    const char *set = builder->variables.items[variable];
    size_t name_length = (size_t)(strchr(set, '=') - set);
    bool later = false;
    for (size_t other = variable + 1; other < builder->variables.count && !later; ++other)
      later = strncmp(builder->variables.items[other], set, name_length + 1) == 0;
    if (!later) entries[count++] = builder->variables.items[variable];
  }
  entries[count] = NULL;
  return entries;
}

static bool connect_stdio(posix_spawn_file_actions_t *actions, int fd, int64_t mode) {
  if (mode == STDIO_INHERIT) return true;
  if (mode == STDIO_NULL)
    return posix_spawn_file_actions_addopen(actions, fd, "/dev/null",
                                            fd == STDIN_FILENO ? O_RDONLY : O_WRONLY, 0) == 0;
  return false;
}

static ChimiBindingResult *spawn_failure(ChimiBindingCall *call, int error, const char *program) {
  char message[512];
  snprintf(message, sizeof(message), "%s: %s", program, strerror(error));
  if (error == ENOENT) return chimi_binding_failure(call, EXEC_NOT_FOUND, message);
  if (error == EACCES || error == ENOEXEC) return chimi_binding_failure(call, EXEC_PERMISSION_DENIED, message);
  return chimi_binding_failure(call, EXEC_FAILED, message);
}

/* native_spawn(builder, stdin, stdout, stderr): starts the command and
   closes the builder. */
ChimiBindingResult *chimi_exec_spawn(ChimiBindingCall *call) {
  Builder *builder = chimi_binding_handle(call, 0, &BUILDER);
  int64_t modes[3];
  if (builder == NULL) return chimi_binding_defect(call, "Exec: the command was already started");
  for (uint32_t index = 0; index < 3; ++index)
    if (!chimi_binding_integer(call, index + 1, &modes[index]) || modes[index] < STDIO_INHERIT ||
        modes[index] > STDIO_NULL)
      return chimi_binding_defect(call, "Exec: an unknown Stdio");
  if (builder->directory != NULL) {
    struct stat info;
    if (stat(builder->directory, &info) != 0 || !S_ISDIR(info.st_mode)) {
      char message[512];
      snprintf(message, sizeof(message), "%s: not a directory", builder->directory);
      ChimiBindingResult *failure = chimi_binding_failure(call, EXEC_INVALID_DIRECTORY, message);
      chimi_binding_close_handle(call, 0, &BUILDER);
      return failure;
    }
  }
  int code = EXEC_FAILED;
  char *path = resolve(builder->program, &code);
  if (path == NULL) {
    char message[512];
    snprintf(message, sizeof(message), "%s: %s", builder->program,
             code == EXEC_PERMISSION_DENIED ? "not executable" : "not found");
    ChimiBindingResult *failure = chimi_binding_failure(call, code, message);
    chimi_binding_close_handle(call, 0, &BUILDER);
    return failure;
  }
  size_t argc = builder->arguments.count;
  char **argv = calloc(argc + 2, sizeof(char *));
  char **envp = child_environment(builder);
  ChildProcess *child = calloc(1, sizeof(ChildProcess));
  posix_spawn_file_actions_t actions;
  posix_spawnattr_t attributes;
  bool actions_ready = posix_spawn_file_actions_init(&actions) == 0;
  bool attributes_ready = posix_spawnattr_init(&attributes) == 0;
  int error = ENOMEM;
  if (argv != NULL && envp != NULL && child != NULL && actions_ready && attributes_ready) {
    argv[0] = builder->program;
    for (size_t index = 0; index < argc; ++index) argv[index + 1] = builder->arguments.items[index];
    sigset_t defaults, empty;
    sigemptyset(&defaults);
    int reset[] = {SIGINT, SIGTERM, SIGHUP, SIGUSR1, SIGUSR2, SIGPIPE, SIGCHLD};
    for (size_t index = 0; index < sizeof(reset) / sizeof(*reset); ++index) sigaddset(&defaults, reset[index]);
    sigemptyset(&empty);
    error = posix_spawnattr_setsigdefault(&attributes, &defaults);
    if (error == 0) error = posix_spawnattr_setsigmask(&attributes, &empty);
    if (error == 0)
      error = posix_spawnattr_setflags(&attributes, POSIX_SPAWN_SETSIGDEF | POSIX_SPAWN_SETSIGMASK);
    for (int fd = 0; fd < 3 && error == 0; ++fd)
      if (!connect_stdio(&actions, fd, modes[fd])) error = EINVAL;
    if (error == 0 && builder->directory != NULL)
      error = posix_spawn_file_actions_addchdir_np(&actions, builder->directory);
    if (error == 0) {
      chimi_binding_prepare_children();
      error = posix_spawn(&child->pid, path, &actions, &attributes, argv, envp);
    }
  }
  if (actions_ready) posix_spawn_file_actions_destroy(&actions);
  if (attributes_ready) posix_spawnattr_destroy(&attributes);
  free(argv);
  free(envp);
  free(path);
  ChimiBindingResult *result;
  if (error != 0) {
    free(child);
    result = spawn_failure(call, error, builder->program);
  } else {
    ChimiBindingValue *handle = chimi_binding_make_handle(call, &CHILD, child);
    result = handle ? chimi_binding_ok(call, handle)
                    : chimi_binding_failure(call, EXEC_FAILED, "too many open children");
  }
  chimi_binding_close_handle(call, 0, &BUILDER);
  return result;
}

/* (0, code) for an exit, (1, signal) for a death by signal. */
static ChimiBindingResult *status_result(ChimiBindingCall *call, int status) {
  ChimiBindingValue *parts[2];
  if (WIFSIGNALED(status)) {
    parts[0] = chimi_binding_make_integer(call, 1);
    parts[1] = chimi_binding_make_integer(call, WTERMSIG(status));
  } else {
    parts[0] = chimi_binding_make_integer(call, 0);
    parts[1] = chimi_binding_make_integer(call, WEXITSTATUS(status));
  }
  return chimi_binding_ok(call, chimi_binding_make_tuple(call, parts, 2));
}

static ChimiBindingResult *closed(ChimiBindingCall *call) {
  return chimi_binding_failure(call, EXEC_CLOSED, "the child was released");
}

/* native_wait(child): its status, waiting for the exit first. */
ChimiBindingResult *chimi_exec_wait(ChimiBindingCall *call) {
  for (;;) {
    ChildProcess *child = chimi_binding_handle(call, 0, &CHILD);
    if (child == NULL) return closed(call);
    if (child->reaped) return status_result(call, child->status);
    int status;
    pid_t reaped = waitpid(child->pid, &status, WNOHANG);
    if (reaped == child->pid) {
      child->reaped = true;
      child->status = status;
      return status_result(call, status);
    }
    if (reaped < 0 && errno != EINTR) {
      char message[256];
      snprintf(message, sizeof(message), "waiting for the child failed: %s", strerror(errno));
      return chimi_binding_failure(call, EXEC_FAILED, message);
    }
    switch (chimi_binding_wait_child(call, 0, child->pid)) {
      case CHIMI_BINDING_WAIT_CANCELLED: return chimi_binding_cancelled(call);
      case CHIMI_BINDING_WAIT_CLOSED: return closed(call);
      default:
        /* Ready, or failed because another task reaped it meanwhile: the
           record answers on the next pass. */
        break;
    }
  }
}

ChimiBindingResult *chimi_exec_terminate(ChimiBindingCall *call) {
  ChildProcess *child = chimi_binding_handle(call, 0, &CHILD);
  if (child != NULL && !child->reaped) kill(child->pid, SIGTERM);
  return chimi_binding_success(call, chimi_binding_void(call));
}

ChimiBindingResult *chimi_exec_kill(ChimiBindingCall *call) {
  ChildProcess *child = chimi_binding_handle(call, 0, &CHILD);
  if (child != NULL && !child->reaped) kill(child->pid, SIGKILL);
  return chimi_binding_success(call, chimi_binding_void(call));
}

/* native_release(child): kills a running child, reaps it, frees the handle. */
ChimiBindingResult *chimi_exec_release(ChimiBindingCall *call) {
  chimi_binding_close_handle(call, 0, &CHILD);
  return chimi_binding_success(call, chimi_binding_void(call));
}

/* native_allow_real_commands() -> Void: test-only (T4401), so a program
   cannot reach `Exec.host_for_tests`. Does nothing. */
ChimiBindingResult *chimi_exec_allow_real_commands(ChimiBindingCall *call) {
  return chimi_binding_success(call, chimi_binding_void(call));
}
