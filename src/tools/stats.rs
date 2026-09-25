//! Small numeric helpers shared by the reports.

/// Median of `values` (sorted in place); 0 for an empty slice.
///
/// Even-length input averages the two middle values. Two reports took
/// `sorted[len / 2]` instead — the upper middle — so an even set of merge times
/// reported a median skewed toward the slow side.
pub(crate) fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.total_cmp(b));
    let n = values.len();
    match n {
        0 => 0.0,
        _ if n % 2 == 1 => values[n / 2],
        _ => (values[n / 2 - 1] + values[n / 2]) / 2.0,
    }
}

#[cfg(test)]
mod tests {
    use super::median;

    #[test]
    fn median_of_odd_even_and_empty_sets() {
        assert_eq!(median(&mut []), 0.0);
        assert_eq!(median(&mut [5.0]), 5.0);
        assert_eq!(median(&mut [9.0, 1.0, 5.0]), 5.0);
        assert_eq!(median(&mut [1.0, 100.0, 2.0, 3.0]), 2.5, "even: mean of the two middle values");
        assert_eq!(median(&mut [f64::NAN, 1.0, 2.0]), 2.0, "NaN sorts last under total_cmp");
    }
}
