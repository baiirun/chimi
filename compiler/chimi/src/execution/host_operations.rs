//! The host core of the system modules (`std/env`, `std/fs`) on the reference
//! interpreter. `runtime.c` implements the same operations in
//! `invoke_host_operation`; `tests/host_modules.rs` runs programs on both and
//! compares them.

use super::{Control, Evaluator, RuntimeValue};
use crate::ids::CoreNodeId;
use crate::standard::{StandardConstructor, StandardFunction};
use std::io::{Read as _, Write as _};
use std::sync::Arc;

/// Bytes per `host_read` chunk, as in `runtime.c`.
const READ_CHUNK_BYTES: usize = 64 * 1024;

/// The files `std/fs` has open. A handle is `slot | generation << 32`, so a
/// handle used after its file was closed names a slot whose generation moved
/// on and fails with the `Closed` code instead of reaching another file.
#[derive(Default)]
pub(super) struct OpenFiles {
    slots: Vec<(u32, Option<std::fs::File>)>,
}

impl OpenFiles {
    fn insert(&mut self, file: std::fs::File) -> i64 {
        let slot = match self.slots.iter().position(|(_, file)| file.is_none()) {
            Some(slot) => slot,
            None => {
                self.slots.push((0, None));
                self.slots.len() - 1
            }
        };
        self.slots[slot].1 = Some(file);
        (slot as i64) | (i64::from(self.slots[slot].0) << 32)
    }

    fn get(&mut self, handle: i64) -> Option<&mut std::fs::File> {
        let (slot, generation) = (handle & 0xffff_ffff, handle >> 32);
        let (current, file) = self.slots.get_mut(usize::try_from(slot).ok()?)?;
        (i64::from(*current) == generation).then_some(file.as_mut()?)
    }

    fn close(&mut self, handle: i64) -> bool {
        if self.get(handle).is_none() {
            return false;
        }
        let entry = &mut self.slots[(handle & 0xffff_ffff) as usize];
        entry.1 = None;
        entry.0 = entry.0.wrapping_add(1);
        true
    }
}

/// The portable failure code of `error`, which `std/fs` maps to `FsError`,
/// and its message: the operating system's text, without the
/// " (os error N)" Rust adds, so both backends report the same words.
fn failure_code(error: &std::io::Error) -> (i64, String) {
    use std::io::ErrorKind;
    let code = match error.kind() {
        ErrorKind::NotFound => -1,
        ErrorKind::PermissionDenied => -2,
        ErrorKind::AlreadyExists => -3,
        ErrorKind::NotADirectory => -4,
        ErrorKind::IsADirectory => -5,
        ErrorKind::DirectoryNotEmpty => -6,
        _ => -8,
    };
    let text = error.to_string();
    let message = match text.rfind(" (os error ") {
        Some(at) => text[..at].to_owned(),
        None => text,
    };
    (code, message)
}

/// The closed-handle failure.
const CLOSED: (i64, &str) = (-7, "file is closed");

fn status(code: i64, message: impl Into<String>) -> RuntimeValue {
    RuntimeValue::Tuple(Arc::new(vec![
        RuntimeValue::Integer(code),
        RuntimeValue::Text(message.into()),
    ]))
}

fn io_status(result: std::io::Result<i64>) -> RuntimeValue {
    match result {
        Ok(value) => status(value, ""),
        Err(error) => {
            let (code, message) = failure_code(&error);
            status(code, message)
        }
    }
}

fn tuple(values: Vec<RuntimeValue>) -> RuntimeValue {
    RuntimeValue::Tuple(Arc::new(values))
}

/// A path's text, or a NUL failure: a C path stops at U+0000, so such a path
/// names no file on either backend.
fn host_path(path: &str) -> std::io::Result<&str> {
    if path.contains('\0') {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path contains a NUL byte",
        ))
    } else {
        Ok(path)
    }
}

impl Evaluator<'_> {
    pub(super) fn invoke_host_operation(
        &mut self,
        function: StandardFunction,
        arguments: Vec<RuntimeValue>,
        call_node: CoreNodeId,
    ) -> Control {
        if self.config.cancellation_requested {
            return Control::Cancellation(None);
        }
        match (function, arguments.as_slice()) {
            (StandardFunction::FsHostOpen, [RuntimeValue::Text(path)]) => {
                Control::Success(io_status(
                    host_path(path)
                        .and_then(std::fs::File::open)
                        .and_then(|file| {
                            if file.metadata()?.is_dir() {
                                return Err(std::io::Error::from(std::io::ErrorKind::IsADirectory));
                            }
                            Ok(self.open_files.insert(file))
                        }),
                ))
            }
            (
                StandardFunction::FsHostCreate,
                [RuntimeValue::Text(path), RuntimeValue::Integer(mode)],
            ) => {
                let mut options = std::fs::OpenOptions::new();
                match mode {
                    0 => options.write(true).create(true).truncate(true),
                    1 => options.append(true).create(true),
                    _ => options.write(true).create_new(true),
                };
                Control::Success(io_status(
                    host_path(path)
                        .and_then(|path| options.open(path))
                        .map(|file| self.open_files.insert(file)),
                ))
            }
            (StandardFunction::FsHostRead, [RuntimeValue::Integer(handle)]) => {
                let Some(file) = self.open_files.get(*handle) else {
                    return Control::Success(tuple(vec![
                        RuntimeValue::Integer(CLOSED.0),
                        RuntimeValue::Bytes(Arc::from(Vec::new())),
                        RuntimeValue::Text(CLOSED.1.to_owned()),
                    ]));
                };
                let mut chunk = vec![0; READ_CHUNK_BYTES];
                let (code, bytes, message) = loop {
                    match file.read(&mut chunk) {
                        Ok(0) => break (0, Vec::new(), String::new()),
                        Ok(count) => {
                            chunk.truncate(count);
                            break (1, chunk, String::new());
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            let (code, message) = failure_code(&error);
                            break (code, Vec::new(), message);
                        }
                    }
                };
                Control::Success(tuple(vec![
                    RuntimeValue::Integer(code),
                    RuntimeValue::Bytes(Arc::from(bytes)),
                    RuntimeValue::Text(message),
                ]))
            }
            (
                StandardFunction::FsHostWrite,
                [RuntimeValue::Integer(handle), RuntimeValue::Bytes(bytes)],
            ) => Control::Success(match self.open_files.get(*handle) {
                Some(file) => io_status(file.write_all(bytes).map(|()| 0)),
                None => status(CLOSED.0, CLOSED.1),
            }),
            (StandardFunction::FsHostClose, [RuntimeValue::Integer(handle)]) => {
                // Dropping the file closes it; a close error is not reported
                // by `File`, so the interpreter flushes nothing it could lose.
                Control::Success(if self.open_files.close(*handle) {
                    status(0, "")
                } else {
                    status(CLOSED.0, CLOSED.1)
                })
            }
            (StandardFunction::FsHostMetadata, [RuntimeValue::Text(path)]) => {
                let metadata = host_path(path).and_then(std::fs::metadata);
                Control::Success(match metadata {
                    Ok(metadata) => {
                        let kind = if metadata.is_file() {
                            0
                        } else if metadata.is_dir() {
                            1
                        } else {
                            2
                        };
                        let modified = metadata
                            .modified()
                            .ok()
                            .and_then(|time| {
                                let (sign, duration) =
                                    match time.duration_since(std::time::UNIX_EPOCH) {
                                        Ok(duration) => (1, duration),
                                        Err(before) => (-1, before.duration()),
                                    };
                                i64::try_from(duration.as_nanos()).ok().map(|n| sign * n)
                            })
                            .unwrap_or(0);
                        tuple(vec![
                            RuntimeValue::Integer(0),
                            RuntimeValue::Integer(kind),
                            RuntimeValue::Integer(
                                i64::try_from(metadata.len()).unwrap_or(i64::MAX),
                            ),
                            RuntimeValue::Integer(modified),
                            RuntimeValue::Text(String::new()),
                        ])
                    }
                    Err(error) => {
                        let (code, message) = failure_code(&error);
                        tuple(vec![
                            RuntimeValue::Integer(code),
                            RuntimeValue::Integer(0),
                            RuntimeValue::Integer(0),
                            RuntimeValue::Integer(0),
                            RuntimeValue::Text(message),
                        ])
                    }
                })
            }
            (StandardFunction::FsHostList, [RuntimeValue::Text(path)]) => {
                let names = host_path(path)
                    .and_then(std::fs::read_dir)
                    .and_then(|entries| {
                        let mut names = Vec::new();
                        for entry in entries {
                            match entry?.file_name().into_string() {
                                Ok(name) => names.push(name),
                                Err(_) => {
                                    return Err(std::io::Error::other("a name is not UTF-8"));
                                }
                            }
                        }
                        names.sort_unstable();
                        Ok(names)
                    });
                Control::Success(match names {
                    Ok(names) => tuple(vec![
                        RuntimeValue::Integer(0),
                        RuntimeValue::List(names.into_iter().map(RuntimeValue::Text).collect()),
                        RuntimeValue::Text(String::new()),
                    ]),
                    Err(error) => {
                        let (code, message) = failure_code(&error);
                        tuple(vec![
                            RuntimeValue::Integer(code),
                            RuntimeValue::List(Default::default()),
                            RuntimeValue::Text(message),
                        ])
                    }
                })
            }
            (StandardFunction::FsHostCreateDirectory, [RuntimeValue::Text(path)]) => {
                Control::Success(io_status(
                    host_path(path).and_then(std::fs::create_dir).map(|()| 0),
                ))
            }
            (StandardFunction::FsHostRemove, [RuntimeValue::Text(path)]) => {
                Control::Success(io_status(host_path(path).and_then(|path| {
                    if std::fs::symlink_metadata(path)?.is_dir() {
                        std::fs::remove_dir(path)
                    } else {
                        std::fs::remove_file(path)
                    }
                    .map(|()| 0)
                })))
            }
            (
                StandardFunction::FsHostRename,
                [RuntimeValue::Text(from), RuntimeValue::Text(to)],
            ) => Control::Success(io_status(
                host_path(from)
                    .and_then(|from| Ok((from, host_path(to)?)))
                    .and_then(|(from, to)| std::fs::rename(from, to))
                    .map(|()| 0),
            )),
            (StandardFunction::FsHostIsDirectory, [RuntimeValue::Text(path)]) => {
                Control::Success(io_status(
                    host_path(path)
                        .and_then(std::fs::symlink_metadata)
                        .map(|metadata| i64::from(metadata.is_dir())),
                ))
            }
            (StandardFunction::FsHostTemporaryDirectory, []) => match make_temporary_directory() {
                Ok(path) => Control::Success(RuntimeValue::Text(path)),
                Err(error) => Control::Defect {
                    message: format!("cannot create a temporary directory: {error}"),
                    node: Some(call_node),
                    cause: None,
                },
            },
            (StandardFunction::FsTestRoot, [RuntimeValue::Text(root)]) => {
                if root.starts_with('/') {
                    Control::Success(RuntimeValue::Text(root.clone()))
                } else {
                    Control::Defect {
                        message: format!("Fs.files_within needs an absolute root, not {root:?}"),
                        node: Some(call_node),
                        cause: None,
                    }
                }
            }
            (StandardFunction::SignalHostInstall, []) => Control::Success(RuntimeValue::Void),
            (StandardFunction::SignalHostWait, [RuntimeValue::List(codes)]) => Control::Defect {
                message: if codes.len() == 0 {
                    "Signal.next needs at least one signal to wait for".to_owned()
                } else {
                    "Signal.next requires the native runtime; build the program with `chimi build`"
                        .to_owned()
                },
                node: Some(call_node),
                cause: None,
            },
            (_, []) => self.invoke_environment_operation(function, call_node),
            _ => self.intrinsic_shape_defect(function, call_node),
        }
    }

    fn invoke_environment_operation(
        &self,
        function: StandardFunction,
        call_node: CoreNodeId,
    ) -> Control {
        match function {
            // `vars_os` reads `environ` in order, as the native runtime does;
            // a pair that is not UTF-8 is left out on both, and a name may start
            // with `=` (the separator is the first `=` after the first byte).
            StandardFunction::EnvHostVariables => Control::Success(RuntimeValue::List(
                std::env::vars_os()
                    .filter_map(|(name, value)| {
                        Some((name.into_string().ok()?, value.into_string().ok()?))
                    })
                    .map(|(name, value)| {
                        RuntimeValue::Tuple(Arc::new(vec![
                            RuntimeValue::Text(name),
                            RuntimeValue::Text(value),
                        ]))
                    })
                    .collect(),
            )),
            StandardFunction::EnvHostCurrentDirectory => Control::Success(
                match std::env::current_dir()
                    .ok()
                    .and_then(|path| path.into_os_string().into_string().ok())
                {
                    Some(path) => self.standard_value(
                        StandardConstructor::OptionSome,
                        vec![RuntimeValue::Text(path)],
                    ),
                    None => self.standard_value(StandardConstructor::OptionNone, Vec::new()),
                },
            ),
            _ => self.intrinsic_shape_defect(function, call_node),
        }
    }
}

/// A new directory under the system's temporary directory, named as
/// `mkdtemp` names the native runtime's: `chimi-test-` and six characters.
fn make_temporary_directory() -> std::io::Result<String> {
    let base = std::env::temp_dir();
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos())
        ^ u128::from(std::process::id());
    for attempt in 0..1000_u128 {
        let mut value = seed.wrapping_mul(6364136223846793005).wrapping_add(attempt);
        let suffix: String = (0..6)
            .map(|_| {
                let character = b"abcdefghijklmnopqrstuvwxyz0123456789"[(value % 36) as usize];
                value /= 36;
                char::from(character)
            })
            .collect();
        let path = base.join(format!("chimi-test-{suffix}"));
        match std::fs::create_dir(&path) {
            Ok(()) => {
                return path
                    .canonicalize()?
                    .into_os_string()
                    .into_string()
                    .map_err(|_| std::io::Error::other("temporary directory is not UTF-8"));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other("no free temporary directory name"))
}
