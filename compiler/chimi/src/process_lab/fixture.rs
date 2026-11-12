//! Locating a fixture and comparing its expected files.

use super::catalogue::Scenario;
use std::path::PathBuf;

/// The project root of a fixture scenario.
pub(super) fn root(scenario: &Scenario) -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if scenario.id == "FP01" {
        return manifest.join("../../programs/supervised-pool");
    }
    manifest
        .join("tests/process-lab")
        .join(format!("{}-{}", scenario.id, scenario.slug))
}

/// The lines of an expected file. A missing file fails the scenario: the lab
/// never writes expected files.
pub(super) fn expected_lines(scenario: &Scenario, file: &str) -> Result<Vec<String>, String> {
    let path = root(scenario).join(file);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(text.lines().map(str::to_owned).collect())
}

/// `Ok` when the lists are equal, otherwise the first differing line.
pub(super) fn compare(file: &str, expected: &[String], actual: &[String]) -> Result<(), String> {
    let absent = "<absent>";
    for index in 0..expected.len().max(actual.len()) {
        let left = expected.get(index).map_or(absent, String::as_str);
        let right = actual.get(index).map_or(absent, String::as_str);
        if left != right {
            return Err(format!(
                "{file} line {}: expected `{left}`, actual `{right}`",
                index + 1
            ));
        }
    }
    Ok(())
}

pub(super) fn print_actual(
    scenario: &Scenario,
    file: &str,
    expected: &[String],
    actual: &[String],
) {
    eprintln!("== {} {file} expected", scenario.id);
    for line in expected {
        eprintln!("{line}");
    }
    eprintln!("== {} {file} actual", scenario.id);
    for line in actual {
        eprintln!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    // Rejects a comparison that ignores a missing or extra trailing line.
    #[test]
    fn compare_reports_the_first_differing_line_including_length_differences() {
        assert!(compare("f", &lines(&["a", "b"]), &lines(&["a", "b"])).is_ok());
        assert_eq!(
            compare("f", &lines(&["a", "b"]), &lines(&["a", "c"])).unwrap_err(),
            "f line 2: expected `b`, actual `c`"
        );
        assert_eq!(
            compare("f", &lines(&["a"]), &lines(&["a", "b"])).unwrap_err(),
            "f line 2: expected `<absent>`, actual `b`"
        );
        assert!(compare("f", &lines(&["a", "b"]), &lines(&["a"])).is_err());
    }
}
