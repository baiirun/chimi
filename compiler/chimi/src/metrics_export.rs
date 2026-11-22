//! `chimi metrics export --otlp`: a run's `chimi.metrics.v1` document
//! (observability.md, "The runner's aggregate") as OpenTelemetry metrics, in
//! the OTLP/JSON encoding of `MetricsData`.
//!
//! Each name is one metric, and each of its label sets is one data point,
//! cumulative from the run's start to the end of `main`. A count is a
//! monotonic sum, a gauge is a gauge, and an observation is an explicit-bucket
//! histogram with the document's bounds.

use crate::json::{JsonData, decode_json, write_json_string};
use crate::trace_view::Record;
use std::fmt::Write;

const SCHEMA: &str = "chimi.metrics.v1";
/// AGGREGATION_TEMPORALITY_CUMULATIVE.
const CUMULATIVE: u32 = 2;

/// A metrics document that cannot be converted, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricsError(pub String);

impl std::fmt::Display for MetricsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The OTLP/JSON document for the metrics document `text`. `service_name`
/// names the resource; `unknown_service` is used when it is `None`, as
/// OpenTelemetry does for an unnamed service.
pub fn export_metrics_otlp(text: &str, service_name: Option<&str>) -> Result<String, MetricsError> {
    let error = |message: &str| MetricsError(message.to_owned());
    let JsonData::Object(fields) =
        decode_json(text.trim_end()).map_err(|_| error("not a JSON document"))?
    else {
        return Err(error("not a JSON object"));
    };
    let document = Record(&fields);
    if document.text("schema") != Some(SCHEMA) {
        return Err(error("not a chimi.metrics.v1 document"));
    }
    let time = |key: &str| match document.get(key) {
        Some(JsonData::Number(number)) if number.parse::<u64>().is_ok() => Ok(number.as_str()),
        _ => Err(MetricsError(format!(
            "the document has no {key}; it was written by a runtime without run times"
        ))),
    };
    let times = Times {
        start: time("start_unix_ns")?,
        end: time("time_unix_ns")?,
    };
    let Some(JsonData::Array(series)) = document.get("metrics") else {
        return Err(error("the document has no metrics list"));
    };
    let mut metrics = Vec::new();
    for (index, entry) in series.iter().enumerate() {
        let JsonData::Object(fields) = entry else {
            return Err(MetricsError(format!("metric {index} is not an object")));
        };
        let series = Series::read(Record(fields))
            .ok_or_else(|| MetricsError(format!("metric {index} is not a count, gauge or observation")))?;
        metrics.push(series);
    }

    let mut output = String::from("{\"resourceMetrics\":[{\"resource\":{\"attributes\":[");
    output.push_str("{\"key\":\"service.name\",\"value\":{\"stringValue\":");
    write_json_string(&mut output, service_name.unwrap_or("unknown_service"));
    output.push_str("}}]},\"scopeMetrics\":[{\"scope\":{\"name\":\"chimi\",\"version\":");
    write_json_string(&mut output, env!("CARGO_PKG_VERSION"));
    output.push_str("},\"metrics\":[");
    let mut first = true;
    // The document's series are ordered by name, so one name's label sets
    // are adjacent.
    for group in metrics.chunk_by(|left, right| left.name == right.name && left.kind == right.kind) {
        if !first {
            output.push(',');
        }
        first = false;
        write_metric(&mut output, group, &times);
    }
    for (key, name) in [
        ("label_sets", "chimi.metrics.dropped_label_sets"),
        ("kind_conflicts", "chimi.metrics.dropped_kind_conflicts"),
    ] {
        let dropped = match document.get("dropped") {
            Some(JsonData::Object(dropped)) => Record(dropped).number(key).unwrap_or(0),
            _ => 0,
        };
        if dropped > 0 {
            if !first {
                output.push(',');
            }
            first = false;
            let series = Series {
                name: name.to_owned(),
                labels: Vec::new(),
                kind: Kind::Count,
                value: dropped.to_string(),
                histogram: None,
            };
            write_metric(&mut output, std::slice::from_ref(&series), &times);
        }
    }
    output.push_str("]}]}]}\n");
    Ok(output)
}

struct Times<'a> {
    start: &'a str,
    end: &'a str,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Count,
    Gauge,
    Observe,
}

struct Series {
    name: String,
    labels: Vec<(String, String)>,
    kind: Kind,
    /// A count's or gauge's value, as the document spells it.
    value: String,
    /// An observation's count, sum, bounds and bucket counts.
    histogram: Option<Histogram>,
}

struct Histogram {
    count: String,
    sum: String,
    bounds: Vec<String>,
    buckets: Vec<String>,
}

fn number(value: Option<&JsonData>) -> Option<String> {
    match value? {
        JsonData::Number(number) => Some(number.clone()),
        _ => None,
    }
}

fn numbers(value: Option<&JsonData>) -> Option<Vec<String>> {
    match value? {
        JsonData::Array(items) => items.iter().map(|item| number(Some(item))).collect(),
        _ => None,
    }
}

impl Series {
    fn read(record: Record<'_>) -> Option<Self> {
        let name = record.text("name")?.to_owned();
        let JsonData::Object(labels) = record.get("labels")? else {
            return None;
        };
        let labels = labels
            .iter()
            .map(|(key, value)| match value {
                JsonData::Text(value) => Some((key.clone(), value.clone())),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        let kind = match record.text("kind")? {
            "count" => Kind::Count,
            "gauge" => Kind::Gauge,
            "observe" => Kind::Observe,
            _ => return None,
        };
        let (value, histogram) = if kind == Kind::Observe {
            let histogram = Histogram {
                count: number(record.get("count"))?,
                sum: number(record.get("sum"))?,
                bounds: numbers(record.get("bounds"))?,
                buckets: numbers(record.get("bucket_counts"))?,
            };
            if histogram.buckets.len() != histogram.bounds.len() + 1 {
                return None;
            }
            (String::new(), Some(histogram))
        } else {
            (number(record.get("value"))?, None)
        };
        Some(Self {
            name,
            labels,
            kind,
            value,
            histogram,
        })
    }
}

/// One metric: `group` is every label set of one name, all of one kind.
fn write_metric(output: &mut String, group: &[Series], times: &Times<'_>) {
    let kind = group[0].kind;
    output.push_str("{\"name\":");
    write_json_string(output, &group[0].name);
    output.push_str(match kind {
        Kind::Count => ",\"sum\":{\"dataPoints\":[",
        Kind::Gauge => ",\"gauge\":{\"dataPoints\":[",
        Kind::Observe => ",\"histogram\":{\"dataPoints\":[",
    });
    for (index, series) in group.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str("{\"attributes\":[");
        for (label, (key, value)) in series.labels.iter().enumerate() {
            if label > 0 {
                output.push(',');
            }
            output.push_str("{\"key\":");
            write_json_string(output, key);
            output.push_str(",\"value\":{\"stringValue\":");
            write_json_string(output, value);
            output.push_str("}}");
        }
        let _ = write!(
            output,
            "],\"startTimeUnixNano\":\"{}\",\"timeUnixNano\":\"{}\"",
            times.start, times.end
        );
        match (kind, &series.histogram) {
            (Kind::Count, _) => {
                let _ = write!(output, ",\"asInt\":\"{}\"", series.value);
            }
            (Kind::Gauge, _) => {
                let _ = write!(output, ",\"asDouble\":{}", series.value);
            }
            (Kind::Observe, Some(histogram)) => {
                let _ = write!(
                    output,
                    ",\"count\":\"{}\",\"sum\":{},\"bucketCounts\":[{}],\"explicitBounds\":[{}]",
                    histogram.count,
                    histogram.sum,
                    histogram
                        .buckets
                        .iter()
                        .map(|bucket| format!("\"{bucket}\""))
                        .collect::<Vec<_>>()
                        .join(","),
                    histogram.bounds.join(","),
                );
            }
            (Kind::Observe, None) => unreachable!("an observation reads with its histogram"),
        }
        output.push('}');
    }
    output.push(']');
    match kind {
        Kind::Count => {
            let _ = write!(output, ",\"aggregationTemporality\":{CUMULATIVE},\"isMonotonic\":true");
        }
        Kind::Gauge => {}
        Kind::Observe => {
            let _ = write!(output, ",\"aggregationTemporality\":{CUMULATIVE}");
        }
    }
    output.push_str("}}");
}
