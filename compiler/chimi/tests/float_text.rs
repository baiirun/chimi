//! Float text against core's `{:e}`: both backends print a Float with
//! `chimi_runtime::float::text`, which generates the shortest round-trip
//! digits itself, so comparing the backends cannot see a wrong digit. core
//! picks them by its own method (Grisu, falling back to exact digits): the
//! shortest that parse back, the closest at that length, and the larger on
//! an exact tie.

/// The significand digits and decimal exponent `text` spells.
fn digits_of(text: &str) -> (String, i32) {
    let text = text.trim_start_matches('-');
    let (mantissa, exponent) = match text.split_once('e') {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().unwrap()),
        None => (text, 0),
    };
    let point = mantissa.find('.').unwrap_or(mantissa.len()) as i32;
    let all: String = mantissa.chars().filter(|c| *c != '.').collect();
    let leading = all.len() - all.trim_start_matches('0').len();
    let digits = all.trim_start_matches('0').trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    (digits.to_owned(), exponent + point - 1 - leading as i32)
}

fn core_digits(magnitude: f64) -> (String, i32) {
    let written = format!("{magnitude:e}");
    let (significand, exponent) = written.split_once('e').unwrap();
    (significand.replace('.', ""), exponent.parse().unwrap())
}

#[test]
fn float_text_has_core_s_shortest_digits_for_every_kind_of_double() {
    let mut values = vec![5e-324, f64::MIN_POSITIVE, f64::MAX, 1.0, 0.3, 0.5, 9007199254740993.0, 1e23, 8.5, 2.5e-7];
    // Powers of two, where the interval below a value is half as wide, the
    // doubles next to them, and random bit patterns across the whole range.
    for power in -1074..1024 {
        let value = 2f64.powi(power);
        values.extend([value, f64::from_bits(value.to_bits() + 1), f64::from_bits(value.to_bits().saturating_sub(1))]);
    }
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..200_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        values.push(f64::from_bits(state & !(1 << 63)));
        // Short decimals, whose digits end in a carry or a tie more often.
        values.push((state % 1_000_000) as f64 / 10f64.powi((state >> 40) as i32 % 12));
    }
    for value in values.into_iter().filter(|value| value.is_finite() && *value > 0.0) {
        let printed = chimi_runtime::float::text(value);
        assert_eq!(digits_of(printed.as_str()), core_digits(value), "{value:e} printed as {}", printed.as_str());
    }
}
