//! Per-frame stage times in constant memory. `play` runs as a service for
//! days; a vector of every frame's time sorted at exit grew by ~80 MB a day
//! on a 59.94 Hz home screen, and was only freed when the run stopped.

/// Bucket width, ms: the summary prints two decimals, so a quantile read from
/// the buckets is off by at most what the summary shows.
const STEP: f64 = 0.01;
/// Buckets from 0 to 100 ms. Longer times are counted, and set the maximum.
const BUCKETS: usize = 10_000;

/// A histogram of one stage's times, milliseconds.
#[derive(Default)]
pub struct Times {
    /// Allocated on the first time, so a stage that never runs costs
    /// nothing.
    counts: Vec<u64>,
    /// Times beyond the last bucket.
    over: u64,
    n: u64,
    max: f64,
    first: Option<f64>,
    last: Option<f64>,
}

impl Times {
    pub fn push(&mut self, ms: f64) {
        let ms = ms.max(0.0);
        if self.counts.is_empty() {
            self.counts = vec![0; BUCKETS];
        }
        match self.counts.get_mut((ms / STEP) as usize) {
            Some(c) => *c += 1,
            None => self.over += 1,
        }
        self.n += 1;
        self.max = self.max.max(ms);
        self.first.get_or_insert(ms);
        self.last = Some(ms);
    }

    pub fn first(&self) -> Option<f64> {
        self.first
    }

    pub fn last(&self) -> Option<f64> {
        self.last
    }

    /// The time at quantile `q` (0 to 1), taken at the same rank as from a
    /// sorted vector, `(n - 1) * q` rounded down: the top of its bucket, so
    /// never below the true value and at most [`STEP`] above it. A rank among
    /// the times beyond the buckets reads as the maximum.
    pub fn quantile(&self, q: f64) -> Option<f64> {
        if self.n == 0 {
            return None;
        }
        let rank = ((self.n - 1) as f64 * q) as u64;
        let mut seen = 0;
        for (i, &c) in self.counts.iter().enumerate() {
            seen += c;
            if rank < seen {
                // The maximum is exact, and caps the bucket's top.
                return Some(((i + 1) as f64 * STEP).min(self.max));
            }
        }
        Some(self.max)
    }

    /// `name: median …, p95 …, max …`, as the run summary prints it.
    pub fn line(&self, name: &str) -> String {
        match (self.quantile(0.5), self.quantile(0.95)) {
            (Some(median), Some(p95)) => format!(
                "{name}: median {median:.2} ms, p95 {p95:.2}, max {:.2}\n",
                self.max
            ),
            _ => format!("{name}: none\n"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic samples: a frame-time-like spread with a long tail.
    fn samples(n: usize) -> Vec<f64> {
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let u = (x >> 11) as f64 / (1u64 << 53) as f64;
                // Mostly 1-3 ms, sometimes up to ~40.
                1.0 + 2.0 * u + if x.is_multiple_of(97) { 40.0 * u } else { 0.0 }
            })
            .collect()
    }

    #[test]
    fn quantiles_match_a_sorted_vector_within_a_bucket() {
        let v = samples(100_000);
        let mut t = Times::default();
        for &ms in &v {
            t.push(ms);
        }
        let mut sorted = v.clone();
        sorted.sort_by(f64::total_cmp);
        for q in [0.0, 0.25, 0.5, 0.9, 0.95, 0.99, 1.0] {
            let exact = sorted[((sorted.len() - 1) as f64 * q) as usize];
            let got = t.quantile(q).unwrap();
            assert!(
                got >= exact && got - exact <= STEP,
                "q {q}: {got} against {exact}"
            );
        }
        assert_eq!(t.quantile(1.0), Some(*sorted.last().unwrap()));
        assert_eq!(t.first(), Some(v[0]));
        assert_eq!(t.last(), Some(*v.last().unwrap()));
    }

    #[test]
    fn memory_stays_the_same_over_a_million_times() {
        let mut t = Times::default();
        for ms in samples(1_000_000) {
            t.push(ms);
        }
        assert_eq!(t.counts.len(), BUCKETS);
        assert_eq!(t.n, 1_000_000);
    }

    #[test]
    fn times_beyond_the_buckets_are_counted_and_set_the_max() {
        let mut t = Times::default();
        for ms in [1.0, 2.0, 250.0, 300.0] {
            t.push(ms);
        }
        assert_eq!(t.over, 2);
        assert_eq!(t.quantile(1.0), Some(300.0));
        // Rank 2 of 4 is among the long ones: read as the maximum.
        assert_eq!(t.quantile(0.95), Some(300.0));
        assert!((t.quantile(0.0).unwrap() - 1.01).abs() < 1e-9);
    }

    #[test]
    fn nothing_recorded_says_none() {
        let t = Times::default();
        assert_eq!(t.quantile(0.5), None);
        assert_eq!(t.line("render"), "render: none\n");
    }
}
