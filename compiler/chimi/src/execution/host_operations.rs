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
            (StandardFunction::FsHostOpen, [RuntimeValue::Text(path)]) => Control::Success(
                io_status(host_path(path).and_then(std::fs::File::open).and_then(|file| {
                    if file.metadata()?.is_dir() {
                        return Err(std::io::Error::from(std::io::ErrorKind::IsADirectory));
                    }
                    Ok(self.open_files.insert(file))
                })),
            ),
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
                            RuntimeValue::Integer(i64::try_from(metadata.len()).unwrap_or(i64::MAX)),
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
                let names = host_path(path).and_then(std::fs::read_dir).and_then(|entries| {
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
            (StandardFunction::FsHostTemporaryDirectory, []) => {
                match make_temporary_directory() {
                    Ok(path) => Control::Success(RuntimeValue::Text(path)),
                    Err(error) => Control::Defect {
                        message: format!("cannot create a temporary directory: {error}"),
                        node: Some(call_node),
                        cause: None,
                    },
                }
            }
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
            (StandardFunction::LogHostMinimumLevel, []) => {
                Control::Success(RuntimeValue::Integer(self.config.log_level))
            }
            (
                StandardFunction::LogHostWrite,
                [
                    RuntimeValue::Integer(level),
                    RuntimeValue::Text(message),
                    RuntimeValue::Text(fields),
                ],
            ) => {
                let line = log_line(*level, message, fields);
                match &self.config.log_file {
                    None => self.application_stderr.push_str(&line),
                    // A log line that cannot be written is lost, as on the
                    // native runner: logging never fails the program.
                    Some(path) => {
                        let _ = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                            .and_then(|mut file| file.write_all(line.as_bytes()));
                    }
                }
                Control::Success(RuntimeValue::Void)
            }
            (StandardFunction::MetricsHostEnabled, []) => {
                Control::Success(RuntimeValue::Boolean(
                    self.config.metrics_file.is_some() || self.metrics.reading,
                ))
            }
            (StandardFunction::MetricsHostDocument, []) => {
                self.metrics.reading = true;
                Control::Success(RuntimeValue::Text(self.metrics.render()))
            }
            (
                StandardFunction::MetricsHostRecord,
                [
                    RuntimeValue::Integer(kind),
                    RuntimeValue::Text(name),
                    RuntimeValue::Float(value),
                    RuntimeValue::Text(labels),
                ],
            ) => {
                self.metrics.record(*kind, name, *value, labels);
                Control::Success(RuntimeValue::Void)
            }
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

/// One `std/log` line as the runner writes it: the wall-clock time, the level,
/// then the message and fields `std/log` already encoded as JSON.
fn log_line(level: i64, message: &str, fields: &str) -> String {
    let level = match level {
        0 => "debug",
        1 => "info",
        2 => "warn",
        _ => "error",
    };
    // Milliseconds since the epoch, negative before 1970, as the native
    // runtime reads CLOCK_REALTIME.
    let millis = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_millis()).unwrap_or(i64::MAX),
    };
    format!(
        "{{\"time\":\"{}\",\"level\":\"{level}\",\"message\":{message},\"fields\":{fields}}}\n",
        utc_millis(millis)
    )
}

/// RFC 3339 UTC with milliseconds, `2026-11-22T06:00:00.000Z`, by the proleptic
/// Gregorian calendar.
fn utc_millis(millis_since_epoch: i64) -> String {
    let seconds = millis_since_epoch.div_euclid(1000);
    let millis = millis_since_epoch.rem_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    // civil_from_days (Howard Hinnant), as the native runtime's timestamp text.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60
    )
}

#[cfg(test)]
mod log_line_tests {
    #[test]
    fn utc_millis_follows_the_calendar() {
        assert_eq!(super::utc_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(super::utc_millis(951_782_400_007), "2000-02-29T00:00:00.007Z");
        assert_eq!(super::utc_millis(1_795_327_200_250), "2026-11-22T06:00:00.250Z");
        assert_eq!(super::utc_millis(-750), "1969-12-31T23:59:59.250Z");
        assert_eq!(super::utc_millis(10_000_000_000_250), "2286-11-20T17:46:40.250Z");
    }
}

/// Most label sets one metric name keeps; a measurement with a further one is
/// dropped and counted, so a label holding an id cannot grow without bound.
pub(super) const METRIC_SERIES_PER_NAME: usize = 1000;

/// Upper bounds of an observed metric's histogram buckets, in seconds when the
/// value is a duration (OpenTelemetry's default boundaries).
pub(super) const METRIC_BOUNDS: [f64; 15] = [
    0.0, 0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

#[derive(Clone, Debug)]
enum Series {
    Count(i64),
    Gauge(f64),
    Observe { count: u64, sum: f64, buckets: [u64; METRIC_BOUNDS.len() + 1] },
}

/// What the runner's `std/metrics` provider has recorded: one aggregate per
/// name and label set, in name and then label order.
#[derive(Default)]
pub(super) struct MetricsAggregate {
    /// The kind code each name was first recorded with.
    kinds: std::collections::BTreeMap<String, i64>,
    series: std::collections::BTreeMap<(String, String), Series>,
    series_per_name: std::collections::BTreeMap<String, usize>,
    dropped_label_sets: u64,
    dropped_kind_conflicts: u64,
    /// Set by the first `Metrics.runner_document`: from then on the provider
    /// records without `--metrics-file`.
    reading: bool,
}

impl MetricsAggregate {
    /// Adds one measurement: `kind` 0 count, 1 gauge, 2 observe; `name` is the
    /// metric's name as a JSON string and `labels` the label set as a JSON
    /// object, together its key.
    pub(super) fn record(&mut self, kind: i64, name: &str, value: f64, labels: &str) {
        if *self.kinds.entry(name.to_owned()).or_insert(kind) != kind {
            self.dropped_kind_conflicts += 1;
            return;
        }
        let key = (name.to_owned(), labels.to_owned());
        if !self.series.contains_key(&key) {
            let count = self.series_per_name.entry(name.to_owned()).or_default();
            if *count == METRIC_SERIES_PER_NAME {
                self.dropped_label_sets += 1;
                return;
            }
            *count += 1;
            let empty = match kind {
                0 => Series::Count(0),
                1 => Series::Gauge(0.0),
                _ => Series::Observe {
                    count: 0,
                    sum: 0.0,
                    buckets: [0; METRIC_BOUNDS.len() + 1],
                },
            };
            self.series.insert(key.clone(), empty);
        }
        match self.series.get_mut(&key).expect("the series was just made") {
            // A count's value is a whole, non-negative number (std/metrics.count).
            Series::Count(total) => *total = total.saturating_add(value as i64),
            Series::Gauge(last) => *last = value,
            Series::Observe { count, sum, buckets } => {
                *count += 1;
                // A sum past the largest Float saturates there, so it stays finite.
                let total = *sum + value;
                *sum = if total.is_finite() { total } else { f64::MAX.copysign(total) };
                let bucket = METRIC_BOUNDS
                    .iter()
                    .position(|bound| value <= *bound)
                    .unwrap_or(METRIC_BOUNDS.len());
                buckets[bucket] += 1;
            }
        }
    }

    /// The metrics file: one JSON document, as the native runtime writes it.
    pub(super) fn render(&self) -> String {
        let series = self
            .series
            .iter()
            .map(|((metric, labels), series)| {
                let head = format!("{{\"name\":{metric},\"labels\":{labels},");
                match series {
                    Series::Count(total) => {
                        format!("{head}\"kind\":\"count\",\"value\":{total}}}")
                    }
                    Series::Gauge(last) => format!(
                        "{head}\"kind\":\"gauge\",\"value\":{}}}",
                        crate::numeric::render_float(*last)
                    ),
                    Series::Observe { count, sum, buckets } => format!(
                        "{head}\"kind\":\"observe\",\"count\":{count},\"sum\":{},\"bounds\":[{}],\"bucket_counts\":[{}]}}",
                        crate::numeric::render_float(*sum),
                        METRIC_BOUNDS
                            .iter()
                            .map(|bound| crate::numeric::render_float(*bound))
                            .collect::<Vec<_>>()
                            .join(","),
                        buckets
                            .iter()
                            .map(u64::to_string)
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            // The interpreter has no clock: the run starts and ends at Unix 0,
            // as under the native virtual clock without `--clock-start`.
            "{{\"schema\":\"chimi.metrics.v1\",\"metrics\":[{series}],\"dropped\":{{\"label_sets\":{},\"kind_conflicts\":{}}},\"start_unix_ns\":0,\"time_unix_ns\":0}}\n",
            self.dropped_label_sets, self.dropped_kind_conflicts
        )
    }
}
