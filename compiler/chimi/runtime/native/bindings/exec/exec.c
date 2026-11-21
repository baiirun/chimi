/* std/exec: child processes over the binding API (G20). A command is built
   on a Builder handle one piece at a time, then started with posix_spawn.
   The program is looked up on the parent's PATH here, before the spawn, so a
   missing or non-executable program fails at once with its own code, and
   the path found is made absolute, so a relative one still names the same
   file after the child changes directory. The child starts with an empty
   signal mask. Its signal actions are what exec leaves: the runtime's
   handlers reset to the default, and an action the parent inherited as
   ignored (SIGHUP under nohup) stays ignored, as a shell would leave it.

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
#include <pthread.h>
#include <time.h>
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
#if defined(__linux__)
#include <sys/timerfd.h>
#elif defined(__APPLE__) || defined(__FreeBSD__)
#include <sys/event.h>
#endif
#include <unistd.h>

extern char **environ;

/* The failure codes the Chimi wrapper maps to `ExecError`. */
enum {
  EXEC_NOT_FOUND = 1,
  EXEC_PERMISSION_DENIED = 2,
  EXEC_INVALID_DIRECTORY = 3,
  EXEC_CLOSED = 4,
  EXEC_FAILED = 5,
  /* A write to a pipe the child no longer reads. */
  EXEC_BROKEN_PIPE = 6,
};

/* How a standard stream of the child is connected (std/exec `Stdio`). */
enum { STDIO_INHERIT = 0, STDIO_NULL = 1, STDIO_PIPE = 2 };

/* The bytes one read of a pipe returns at most. */
#define PIPE_CHUNK 65536

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
   and no zombie stays behind. `Exec.start` stops the child first with
   chimi_exec_stop, which waits parked, so this reap finds it reaped; it
   blocks only when that could not finish, and at the program's end, for a
   child whose handle outlived every start (the exit backstop). */
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

/* The parent's end of a pipe to one of the child's standard streams. */
typedef struct {
  int fd;
} Pipe;

static void release_pipe(void *state) {
  Pipe *pipe_state = state;
  close(pipe_state->fd);
  free(pipe_state);
}

static const ChimiBindingType BUILDER = {"Command", release_builder};
static const ChimiBindingType CHILD = {"Child", release_child};
static const ChimiBindingType PIPE = {"Pipe", release_pipe};

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

/* `path` made absolute against the parent's working directory; frees `path`.
   NULL with errno set when out of memory or the directory cannot be named. */
static char *absolute(char *path) {
  if (path == NULL || path[0] == '/') return path;
  char *directory = getcwd(NULL, 0);
  char *joined = directory ? malloc(strlen(directory) + strlen(path) + 2) : NULL;
  int error = errno;
  if (joined != NULL) sprintf(joined, "%s/%s", directory, path);
  free(directory);
  free(path);
  errno = error;
  return joined;
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
    if (executable(program, &error)) {
      char *found = absolute(strdup(program));
      if (found == NULL) *code = EXEC_FAILED;
      return found;
    }
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
    if (executable(candidate, &error)) {
      char *found = absolute(candidate);
      if (found == NULL) *code = EXEC_FAILED;
      return found;
    }
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

/* `fd` moved above the standard streams and closed in every child. */
static int private_descriptor(int fd) {
  int moved = fcntl(fd, F_DUPFD_CLOEXEC, STDERR_FILENO + 1);
  int error = errno;
  close(fd);
  errno = error;
  return moved;
}

/* A pipe for the child's stream `target`: ends[0] the parent's, ends[1] the
   child's, both closed on exec, so no other child ever holds them. The
   parent's end does not block, and a write to it after the child stopped
   reading fails with EPIPE where the platform allows (F_SETNOSIGPIPE)
   instead of raising SIGPIPE. 0, or an errno value. */
static int open_pipe(int target, int ends[2]) {
  int raw[2];
#ifdef __linux__
  if (pipe2(raw, O_CLOEXEC) != 0) return errno;
#else
  if (pipe(raw) != 0) return errno;
#endif
  int read_end = private_descriptor(raw[0]);
  int write_end = read_end < 0 ? -1 : private_descriptor(raw[1]);
  if (read_end < 0 || write_end < 0) {
    int error = errno;
    if (read_end >= 0) close(read_end);
    else close(raw[1]);
    return error;
  }
  ends[0] = target == STDIN_FILENO ? write_end : read_end;
  ends[1] = target == STDIN_FILENO ? read_end : write_end;
  int flags = fcntl(ends[0], F_GETFL);
  bool ready = flags >= 0 && fcntl(ends[0], F_SETFL, flags | O_NONBLOCK) == 0;
#ifdef F_SETNOSIGPIPE
  /* write_quietly relies on it: without it a write would raise SIGPIPE. */
  if (ready && target == STDIN_FILENO) ready = fcntl(ends[0], F_SETNOSIGPIPE, 1) == 0;
#endif
  if (!ready) {
    int error = errno;
    close(ends[0]);
    close(ends[1]);
    ends[0] = ends[1] = -1;
    return error;
  }
  return 0;
}

/* A child holds its three standard streams and no other descriptor of the
   program's, even one another carrier thread has just opened and not yet
   marked close-on-exec, where that takes a second call (sockets and pipes
   off Linux). macOS closes every descriptor the file actions do not name
   (POSIX_SPAWN_CLOEXEC_DEFAULT), so an inherited stream is named; glibc
   closes those above standard error as the last action. */
#if defined(__APPLE__)
#define SPAWN_FLAGS (POSIX_SPAWN_SETSIGMASK | POSIX_SPAWN_CLOEXEC_DEFAULT)
#else
#define SPAWN_FLAGS POSIX_SPAWN_SETSIGMASK
#endif

static int close_other_descriptors(posix_spawn_file_actions_t *actions) {
#if defined(__GLIBC__)
#if __GLIBC_PREREQ(2, 34)
  return posix_spawn_file_actions_addclosefrom_np(actions, STDERR_FILENO + 1);
#endif
#endif
  (void)actions;
  return 0;
}

static bool connect_stdio(posix_spawn_file_actions_t *actions, int fd, int64_t mode, int child_end) {
#if defined(__APPLE__)
  if (mode == STDIO_INHERIT) return posix_spawn_file_actions_addinherit_np(actions, fd) == 0;
#else
  if (mode == STDIO_INHERIT) return true;
#endif
  if (mode == STDIO_NULL)
    return posix_spawn_file_actions_addopen(actions, fd, "/dev/null",
                                            fd == STDIN_FILENO ? O_RDONLY : O_WRONLY, 0) == 0;
  if (mode == STDIO_PIPE) return posix_spawn_file_actions_adddup2(actions, child_end, fd) == 0;
  return false;
}

/* A failure of posix_spawn itself. The program and directory were checked
   before it, so ENOENT and EACCES here mean they changed in between, or the
   program's interpreter (a script's #! line) is missing or not executable;
   ENOEXEC is a file the kernel cannot run, which is not a permission. The
   message is the operating system's words alone: `ExecError` names the
   program. */
static ChimiBindingResult *spawn_failure(ChimiBindingCall *call, int error) {
  int code = error == ENOENT ? EXEC_NOT_FOUND : error == EACCES ? EXEC_PERMISSION_DENIED : EXEC_FAILED;
  return chimi_binding_failure(call, code, strerror(error));
}

/* The spawned child and its pipes as handles. If one cannot be made (the
   handle table is full), the child is killed and reaped at once; a pipe
   handle made before that is closed when its value is freed. */
static ChimiBindingResult *started(ChimiBindingCall *call, ChildProcess *child, int ends[3][2]) {
  ChimiBindingValue *parts[4];
  bool made = true;
  for (int fd = 0; fd < 3; ++fd) {
    parts[fd + 1] = chimi_binding_none(call);
    if (ends[fd][0] < 0) continue;
    Pipe *pipe_state = made ? malloc(sizeof(Pipe)) : NULL;
    if (pipe_state == NULL) {
      close(ends[fd][0]);
      made = false;
      continue;
    }
    pipe_state->fd = ends[fd][0];
    ChimiBindingValue *handle = chimi_binding_make_handle(call, &PIPE, pipe_state);
    if (handle == NULL) made = false;
    else parts[fd + 1] = chimi_binding_some(call, handle);
  }
  if (!made) {
    release_child(child);
    return chimi_binding_failure(call, EXEC_FAILED, strerror(EMFILE));
  }
  /* With no handle made, chimi_binding_make_handle has already released
     the child, which killed and reaped it. */
  parts[0] = chimi_binding_make_handle(call, &CHILD, child);
  if (parts[0] == NULL) return chimi_binding_failure(call, EXEC_FAILED, strerror(EMFILE));
  return chimi_binding_ok(call, chimi_binding_make_tuple(call, parts, 4));
}

/* native_spawn(builder, stdin, stdout, stderr) -> Result<(Process,
   Option<Pipe>, Option<Pipe>, Option<Pipe>), (Int, Text)>: starts the
   command and closes the builder, with a pipe for each stream given
   Stdio.Pipe. */
ChimiBindingResult *chimi_exec_spawn(ChimiBindingCall *call) {
  Builder *builder = chimi_binding_handle(call, 0, &BUILDER);
  int64_t modes[3];
  if (builder == NULL) return chimi_binding_defect(call, "Exec: the command was already started");
  for (uint32_t index = 0; index < 3; ++index)
    if (!chimi_binding_integer(call, index + 1, &modes[index]) || modes[index] < STDIO_INHERIT ||
        modes[index] > STDIO_PIPE)
      return chimi_binding_defect(call, "Exec: an unknown Stdio");
  if (builder->directory != NULL) {
    /* A directory the child could not enter (no search permission) is
       invalid too, not a failure found only inside the child. */
    struct stat info;
    if (stat(builder->directory, &info) != 0 || !S_ISDIR(info.st_mode) || access(builder->directory, X_OK) != 0) {
      ChimiBindingResult *failure = chimi_binding_failure(call, EXEC_INVALID_DIRECTORY, "not a directory it can enter");
      chimi_binding_close_handle(call, 0, &BUILDER);
      return failure;
    }
  }
  int code = EXEC_FAILED;
  errno = 0;
  char *path = resolve(builder->program, &code);
  if (path == NULL) {
    int error = code == EXEC_PERMISSION_DENIED ? EACCES : code == EXEC_NOT_FOUND ? ENOENT : errno ? errno : ENOMEM;
    ChimiBindingResult *failure = chimi_binding_failure(call, code, strerror(error));
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
  int ends[3][2] = {{-1, -1}, {-1, -1}, {-1, -1}};
  if (argv != NULL && envp != NULL && child != NULL && actions_ready && attributes_ready) {
    error = 0;
    for (int fd = 0; fd < 3 && error == 0; ++fd)
      if (modes[fd] == STDIO_PIPE) error = open_pipe(fd, ends[fd]);
  }
  if (error == 0 && argv != NULL && envp != NULL && child != NULL && actions_ready && attributes_ready) {
    argv[0] = builder->program;
    for (size_t index = 0; index < argc; ++index) argv[index + 1] = builder->arguments.items[index];
    sigset_t empty;
    sigemptyset(&empty);
    error = posix_spawnattr_setsigmask(&attributes, &empty);
    if (error == 0) error = posix_spawnattr_setflags(&attributes, SPAWN_FLAGS);
    for (int fd = 0; fd < 3 && error == 0; ++fd)
      if (!connect_stdio(&actions, fd, modes[fd], ends[fd][1])) error = EINVAL;
    if (error == 0 && builder->directory != NULL)
      error = posix_spawn_file_actions_addchdir_np(&actions, builder->directory);
    if (error == 0) error = close_other_descriptors(&actions);
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
  /* The child holds its ends now; the parent never uses them. */
  for (int fd = 0; fd < 3; ++fd)
    if (ends[fd][1] >= 0) close(ends[fd][1]);
  ChimiBindingResult *result;
  if (error != 0) {
    for (int fd = 0; fd < 3; ++fd)
      if (ends[fd][0] >= 0) close(ends[fd][0]);
    free(child);
    result = spawn_failure(call, error);
  } else {
    result = started(call, child, ends);
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
      return chimi_binding_failure(call, EXEC_FAILED, strerror(errno));
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

/* ---- Stopping ------------------------------------------------------- */

/* How often a stopping child is looked at. The scheduler wakes a child
   waiter only at the exit, so the deadline needs a clock of its own: a
   descriptor that turns readable every tick (timerfd, or a kqueue timer).
   It runs in real time, as the child does, whatever the program's clock. */
#define STOP_TICK_MS 10

static int open_ticker(void) {
#if defined(__linux__)
  int fd = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK | TFD_CLOEXEC);
  if (fd < 0) return -1;
  struct itimerspec every = {{0, STOP_TICK_MS * 1000000L}, {0, STOP_TICK_MS * 1000000L}};
  if (timerfd_settime(fd, 0, &every, NULL) != 0) {
    close(fd);
    return -1;
  }
  return fd;
#elif defined(__APPLE__) || defined(__FreeBSD__)
  int fd = kqueue();
  if (fd < 0) return -1;
  (void)fcntl(fd, F_SETFD, FD_CLOEXEC);
  struct kevent timer;
  EV_SET(&timer, 1, EVFILT_TIMER, EV_ADD | EV_ENABLE, 0, STOP_TICK_MS, NULL);
  if (kevent(fd, &timer, 1, NULL, 0, NULL) != 0) {
    close(fd);
    return -1;
  }
  return fd;
#else
  return -1;
#endif
}

static void drain_ticker(int fd) {
#if defined(__linux__)
  uint64_t ticks;
  (void)!read(fd, &ticks, sizeof(ticks));
#elif defined(__APPLE__) || defined(__FreeBSD__)
  struct kevent fired;
  struct timespec now = {0, 0};
  (void)kevent(fd, NULL, 0, &fired, 1, &now);
#else
  (void)fd;
#endif
}

static int64_t monotonic_ms(void) {
  struct timespec now;
  clock_gettime(CLOCK_MONOTONIC, &now);
  return (int64_t)now.tv_sec * 1000 + now.tv_nsec / 1000000;
}

/* Reaps the child if it has exited; true once it is reaped. */
static bool reap_if_exited(ChildProcess *child) {
  if (child->reaped) return true;
  int status;
  pid_t reaped;
  do {
    reaped = waitpid(child->pid, &status, WNOHANG);
  } while (reaped < 0 && errno == EINTR);
  if (reaped == child->pid) {
    child->reaped = true;
    child->status = status;
  }
  return child->reaped;
}

/* How a stop's wait ended. */
typedef enum { STOP_EXITED, STOP_DEADLINE, STOP_CANCELLED } StopWait;

/* Waits, letting other tasks run, until the child is reaped or `deadline`
   (monotonic ms) passes. */
static StopWait await_exit_until(ChimiBindingCall *call, int ticker, int64_t deadline) {
  for (;;) {
    ChildProcess *child = chimi_binding_handle(call, 0, &CHILD);
    if (child == NULL || reap_if_exited(child)) return STOP_EXITED;
    if (monotonic_ms() >= deadline) return STOP_DEADLINE;
    if (chimi_binding_wait(call, CHIMI_BINDING_NO_HANDLE, ticker, CHIMI_BINDING_READABLE) != CHIMI_BINDING_READY)
      return STOP_CANCELLED;
    drain_ticker(ticker);
  }
}

/* Waits, letting other tasks run, until the child is reaped. It wakes at the
   exit, so no ticker is needed. A wait that does not finish leaves the reap
   to the release that follows. */
static void await_exit(ChimiBindingCall *call) {
  for (;;) {
    ChildProcess *child = chimi_binding_handle(call, 0, &CHILD);
    if (child == NULL || reap_if_exited(child)) return;
    if (chimi_binding_wait_child(call, 0, child->pid) != CHIMI_BINDING_READY) return;
  }
}

/* Past this many milliseconds the grace is as good as forever, and adding it
   to the clock cannot overflow. */
#define STOP_GRACE_LIMIT_MS ((int64_t)1 << 52)

/* native_stop(child, milliseconds) -> Void: a child still running gets
   SIGTERM and `milliseconds` to exit, then SIGKILL, and is reaped; zero
   sends SIGKILL at once, and so does any grace when no ticker can be opened
   (a SIGTERM cut short at once is worse than none). Other tasks run while it
   waits. A cancelled wait (a task ended outright, which no release masks,
   such as a process body forced at its shutdown deadline) must not be
   followed by another, which would park a task that is unwinding: the child gets SIGKILL, the stop answers cancelled, and the
   handle's release at the program's end reaps it. */
ChimiBindingResult *chimi_exec_stop(ChimiBindingCall *call) {
  int64_t grace;
  if (!chimi_binding_integer(call, 1, &grace) || grace < 0)
    return chimi_binding_defect(call, "Exec: a negative stop_within");
  if (grace > STOP_GRACE_LIMIT_MS) grace = STOP_GRACE_LIMIT_MS;
  ChildProcess *child = chimi_binding_handle(call, 0, &CHILD);
  if (child == NULL || reap_if_exited(child)) return chimi_binding_success(call, chimi_binding_void(call));
  StopWait waited = STOP_DEADLINE;
  int ticker = grace > 0 ? open_ticker() : -1;
  if (ticker >= 0) {
    kill(child->pid, SIGTERM);
    waited = await_exit_until(call, ticker, monotonic_ms() + grace);
    close(ticker);
  }
  child = chimi_binding_handle(call, 0, &CHILD);
  if (waited != STOP_EXITED && child != NULL && !child->reaped) {
    kill(child->pid, SIGKILL);
    if (waited == STOP_CANCELLED) return chimi_binding_cancelled(call);
    await_exit(call);
  }
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

/* ---- Pipes ----------------------------------------------------------- */

static ChimiBindingResult *pipe_closed(ChimiBindingCall *call) {
  return chimi_binding_failure(call, EXEC_CLOSED, "the stream was closed");
}

static ChimiBindingResult *pipe_not_ready(ChimiBindingCall *call, int waited) {
  if (waited == CHIMI_BINDING_WAIT_CANCELLED) return chimi_binding_cancelled(call);
  if (waited == CHIMI_BINDING_WAIT_CLOSED) return pipe_closed(call);
  return chimi_binding_failure(call, EXEC_FAILED, strerror(EBADF));
}

/* native_read(pipe) -> Result<Option<Bytes>, (Int, Text)>: the next bytes
   the child wrote, or None at the end, once every writer has closed. */
ChimiBindingResult *chimi_exec_read(ChimiBindingCall *call) {
  /* On the heap: a task's stack is too small to hold a whole chunk. */
  unsigned char *buffer = malloc(PIPE_CHUNK);
  if (buffer == NULL) return chimi_binding_failure(call, EXEC_FAILED, strerror(ENOMEM));
  ChimiBindingResult *result;
  for (;;) {
    Pipe *pipe_state = chimi_binding_handle(call, 0, &PIPE);
    if (pipe_state == NULL) {
      result = pipe_closed(call);
      break;
    }
    ssize_t count = read(pipe_state->fd, buffer, PIPE_CHUNK);
    if (count > 0) {
      result = chimi_binding_ok(call, chimi_binding_some(call, chimi_binding_make_bytes(call, buffer, (size_t)count)));
      break;
    }
    if (count == 0) {
      result = chimi_binding_ok(call, chimi_binding_none(call));
      break;
    }
    if (errno == EINTR) continue;
    if (errno != EAGAIN && errno != EWOULDBLOCK) {
      result = chimi_binding_failure(call, EXEC_FAILED, strerror(errno));
      break;
    }
    int waited = chimi_binding_wait(call, 0, pipe_state->fd, CHIMI_BINDING_READABLE);
    if (waited != CHIMI_BINDING_READY) {
      result = pipe_not_ready(call, waited);
      break;
    }
  }
  free(buffer);
  return result;
}

/* One write that fails with EPIPE, not SIGPIPE, when the child no longer
   reads. Without F_SETNOSIGPIPE the signal is blocked on this thread for
   the write, and one it raised is taken back before unblocking. */
static ssize_t write_quietly(int fd, const unsigned char *bytes, size_t length) {
#ifdef F_SETNOSIGPIPE
  return write(fd, bytes, length);
#else
  sigset_t pipe_signal, previous, pending;
  sigemptyset(&pipe_signal);
  sigaddset(&pipe_signal, SIGPIPE);
  sigpending(&pending);
  bool already = sigismember(&pending, SIGPIPE);
  pthread_sigmask(SIG_BLOCK, &pipe_signal, &previous);
  ssize_t count = write(fd, bytes, length);
  int error = errno;
  if (count < 0 && error == EPIPE && !already) {
    struct timespec now = {0, 0};
    while (sigtimedwait(&pipe_signal, NULL, &now) < 0 && errno == EINTR) {
    }
  }
  pthread_sigmask(SIG_SETMASK, &previous, NULL);
  errno = error;
  return count;
#endif
}

/* native_write(pipe, bytes) -> Result<Void, (Int, Text)>: writes every byte
   or fails; BROKEN_PIPE once the child stopped reading. */
ChimiBindingResult *chimi_exec_write(ChimiBindingCall *call) {
  const unsigned char *bytes;
  size_t length;
  if (!chimi_binding_bytes(call, 1, &bytes, &length)) return chimi_binding_defect(call, "write takes Bytes");
  size_t written = 0;
  while (written < length) {
    Pipe *pipe_state = chimi_binding_handle(call, 0, &PIPE);
    if (pipe_state == NULL) return pipe_closed(call);
    ssize_t count = write_quietly(pipe_state->fd, bytes + written, length - written);
    if (count > 0) {
      written += (size_t)count;
      continue;
    }
    if (count < 0 && errno == EINTR) continue;
    if (count < 0 && errno == EPIPE) return chimi_binding_failure(call, EXEC_BROKEN_PIPE, strerror(EPIPE));
    if (count < 0 && errno != EAGAIN && errno != EWOULDBLOCK)
      return chimi_binding_failure(call, EXEC_FAILED, strerror(errno));
    int waited = chimi_binding_wait(call, 0, pipe_state->fd, CHIMI_BINDING_WRITABLE);
    if (waited != CHIMI_BINDING_READY) return pipe_not_ready(call, waited);
  }
  return chimi_binding_ok(call, chimi_binding_void(call));
}

/* native_close_pipe(pipe) -> Void: closes the parent's end, so the child
   reads the end of its input, or a write to its output fails with EPIPE.
   Tasks waiting on it are woken first and answer Closed. */
ChimiBindingResult *chimi_exec_close_pipe(ChimiBindingCall *call) {
  chimi_binding_close_handle(call, 0, &PIPE);
  return chimi_binding_success(call, chimi_binding_void(call));
}
