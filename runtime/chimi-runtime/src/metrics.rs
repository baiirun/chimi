//! `std/metrics`' host core (observability.md, "The runner's aggregate"): one
//! aggregate per name and label set, written as one `chimi.metrics.v1` JSON
//! document. The interpreter and the native runtime (through
//! `metrics_abi.rs`) both keep their measurements here, so the two write the
//! same document.
//!
//! A name and a label set arrive already encoded as JSON (`std/metrics.host`),
//! so they are keys as they are and are written as they are.

use crate::{TextBuffer, float};
use alloc::collections::BTreeMap;
use alloc::string::String;

/// Most label sets one metric name keeps; a measurement with a further one is
/// dropped and counted, so a label holding an id cannot grow without bound.
pub const SERIES_PER_NAME: usize = 1000;

/// Upper bounds of an observed metric's histogram buckets, in seconds when the
/// value is a duration (OpenTelemetry's default boundaries).
pub const BOUNDS: [f64; 15] = [
    0.0, 0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

/// A measurement's kind code, as `std/metrics.host` passes it: 0 count,
/// 1 gauge, anything else observe.
const COUNT: i64 = 0;
const GAUGE: i64 = 1;

#[derive(Clone, Debug)]
enum Series {
    Count(i64),
    Gauge(f64),
    Observe {
        count: u64,
        sum: f64,
        buckets: [u64; BOUNDS.len() + 1],
    },
}

/// One metric name: the kind it was first recorded with, and its label sets.
#[derive(Debug)]
struct Metric {
    kind: i64,
    series: BTreeMap<String, Series>,
}

/// What the runner's `std/metrics` provider has recorded, in name and then
/// label order (byte order, as the document lists them).
#[derive(Debug, Default)]
pub struct Aggregate {
    metrics: BTreeMap<String, Metric>,
    dropped_label_sets: u64,
    dropped_kind_conflicts: u64,
}

impl Aggregate {
    /// Adds one measurement of `kind` to the series `name` and `labels` name.
    /// A recorded series is found without allocating.
    pub fn record(&mut self, kind: i64, name: &str, value: f64, labels: &str) {
        if !self.metrics.contains_key(name) {
            let metric = Metric {
                kind,
                series: BTreeMap::new(),
            };
            self.metrics.insert(String::from(name), metric);
        }
        let Some(metric) = self.metrics.get_mut(name) else {
            return;
        };
        if metric.kind != kind {
            self.dropped_kind_conflicts += 1;
            return;
        }
        if !metric.series.contains_key(labels) {
            if metric.series.len() == SERIES_PER_NAME {
                self.dropped_label_sets += 1;
                return;
            }
            let empty = match kind {
                COUNT => Series::Count(0),
                GAUGE => Series::Gauge(0.0),
                _ => Series::Observe {
                    count: 0,
                    sum: 0.0,
                    buckets: [0; BOUNDS.len() + 1],
                },
            };
            metric.series.insert(String::from(labels), empty);
        }
        let Some(series) = metric.series.get_mut(labels) else {
            return;
        };
        match series {
            // A count's value is a whole, non-negative number (std/metrics.count).
            Series::Count(total) => *total = total.saturating_add(value as i64),
            Series::Gauge(last) => *last = value,
            Series::Observe { count, sum, buckets } => {
                *count += 1;
                // A sum past the largest Float saturates there, so it stays finite.
                let total = *sum + value;
                *sum = if total.is_finite() { total } else { f64::MAX.copysign(total) };
                let bucket = BOUNDS
                    .iter()
                    .position(|bound| value <= *bound)
                    .unwrap_or(BOUNDS.len());
                buckets[bucket] += 1;
            }
        }
    }

    /// The metrics document, whose counts began at `start_unix_ns` and stand
    /// as at `time_unix_ns`, ending in a newline.
    pub fn document(&self, start_unix_ns: i64, time_unix_ns: i64) -> String {
        let mut out = String::from("{\"schema\":\"chimi.metrics.v1\",\"metrics\":[");
        let mut first = true;
        for (name, metric) in &self.metrics {
            for (labels, series) in &metric.series {
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str("{\"name\":");
                out.push_str(name);
                out.push_str(",\"labels\":");
                out.push_str(labels);
                match series {
                    Series::Count(total) => {
                        out.push_str(",\"kind\":\"count\",\"value\":");
                        push_integer(&mut out, *total);
                    }
                    Series::Gauge(last) => {
                        out.push_str(",\"kind\":\"gauge\",\"value\":");
                        out.push_str(float::text(*last).as_str());
                    }
                    Series::Observe { count, sum, buckets } => {
                        out.push_str(",\"kind\":\"observe\",\"count\":");
                        push_unsigned(&mut out, *count);
                        out.push_str(",\"sum\":");
                        out.push_str(float::text(*sum).as_str());
                        out.push_str(",\"bounds\":[");
                        for (index, bound) in BOUNDS.iter().enumerate() {
                            if index > 0 {
                                out.push(',');
                            }
                            out.push_str(float::text(*bound).as_str());
                        }
                        out.push_str("],\"bucket_counts\":[");
                        for (index, bucket) in buckets.iter().enumerate() {
                            if index > 0 {
                                out.push(',');
                            }
                            push_unsigned(&mut out, *bucket);
                        }
                        out.push(']');
                    }
                }
                out.push('}');
            }
        }
        out.push_str("],\"dropped\":{\"label_sets\":");
        push_unsigned(&mut out, self.dropped_label_sets);
        out.push_str(",\"kind_conflicts\":");
        push_unsigned(&mut out, self.dropped_kind_conflicts);
        out.push_str("},\"start_unix_ns\":");
        push_integer(&mut out, start_unix_ns);
        out.push_str(",\"time_unix_ns\":");
        push_integer(&mut out, time_unix_ns);
        out.push_str("}\n");
        out
    }
}

fn push_unsigned(out: &mut String, value: u64) {
    let mut digits = TextBuffer::<20>::new();
    digits.push_digits(u128::from(value), 1);
    out.push_str(digits.as_str());
}

fn push_integer(out: &mut String, value: i64) {
    if value < 0 {
        out.push('-');
    }
    let mut digits = TextBuffer::<20>::new();
    digits.push_digits(u128::from(value.unsigned_abs()), 1);
    out.push_str(digits.as_str());
}

#[cfg(test)]
mod tests {
    use super::{Aggregate, SERIES_PER_NAME};

    #[test]
    fn the_document_lists_series_in_name_then_label_order() {
        let mut aggregate = Aggregate::default();
        aggregate.record(0, "\"b\"", 2.0, "{}");
        aggregate.record(1, "\"a\"", -1.5, "{\"k\":\"2\"}");
        aggregate.record(1, "\"a\"", 0.25, "{\"k\":\"1\"}");
        aggregate.record(0, "\"b\"", 3.0, "{}");
        aggregate.record(1, "\"b\"", 3.0, "{}");
        aggregate.record(2, "\"c\"", 0.07, "{}");
        aggregate.record(2, "\"c\"", 11.0, "{}");
        assert_eq!(
            aggregate.document(-5, 7),
            concat!(
                "{\"schema\":\"chimi.metrics.v1\",\"metrics\":[",
                "{\"name\":\"a\",\"labels\":{\"k\":\"1\"},\"kind\":\"gauge\",\"value\":0.25},",
                "{\"name\":\"a\",\"labels\":{\"k\":\"2\"},\"kind\":\"gauge\",\"value\":-1.5},",
                "{\"name\":\"b\",\"labels\":{},\"kind\":\"count\",\"value\":5},",
                "{\"name\":\"c\",\"labels\":{},\"kind\":\"observe\",\"count\":2,\"sum\":11.07,",
                "\"bounds\":[0.0,0.005,0.01,0.025,0.05,0.075,0.1,0.25,0.5,0.75,1.0,2.5,5.0,7.5,10.0],",
                "\"bucket_counts\":[0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1]}],",
                "\"dropped\":{\"label_sets\":0,\"kind_conflicts\":1},\"start_unix_ns\":-5,\"time_unix_ns\":7}\n"
            )
        );
    }

    #[test]
    fn a_name_keeps_at_most_its_series_limit_and_counts_saturate() {
        let mut aggregate = Aggregate::default();
        for index in 0..=SERIES_PER_NAME {
            let mut labels = alloc::string::String::from("{\"id\":\"");
            labels.push_str(super::float::text(index as f64).as_str());
            labels.push_str("\"}");
            aggregate.record(0, "\"n\"", 1.0, &labels);
        }
        aggregate.record(0, "\"big\"", 1e300, "{}");
        aggregate.record(0, "\"big\"", 1.0, "{}");
        let document = aggregate.document(0, 0);
        assert!(document.contains("\"label_sets\":1,"), "{document}");
        assert!(document.contains("\"value\":9223372036854775807}"), "{document}");
    }
}
