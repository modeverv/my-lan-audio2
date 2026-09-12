//! Non-real-time aggregation. Never call these allocating methods on capture threads.
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Histogram {
    bins: BTreeMap<u64, u64>,
    count: u64,
    sum: u128,
}

#[derive(Debug, Serialize)]
pub struct Distribution {
    pub count: u64,
    pub min: Option<u64>,
    pub p50: Option<u64>,
    pub p95: Option<u64>,
    pub p99: Option<u64>,
    pub max: Option<u64>,
    pub mean: Option<f64>,
}

impl Histogram {
    pub fn record(&mut self, value: u64) {
        *self.bins.entry(value).or_default() += 1;
        self.count += 1;
        self.sum += value as u128;
    }

    fn percentile(&self, percent: u64) -> Option<u64> {
        let rank = (self.count * percent).div_ceil(100).max(1);
        let mut cumulative = 0;
        self.bins.iter().find_map(|(&value, &count)| {
            cumulative += count;
            (cumulative >= rank).then_some(value)
        })
    }

    pub fn summary(&self) -> Distribution {
        Distribution {
            count: self.count,
            min: self.bins.first_key_value().map(|(&v, _)| v),
            p50: self.percentile(50),
            p95: self.percentile(95),
            p99: self.percentile(99),
            max: self.bins.last_key_value().map(|(&v, _)| v),
            mean: (self.count != 0).then(|| self.sum as f64 / self.count as f64),
        }
    }

    pub fn bins(&self) -> &BTreeMap<u64, u64> {
        &self.bins
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_unknown_not_zero() {
        let d = Histogram::default().summary();
        assert_eq!(d.count, 0);
        assert_eq!(d.p99, None);
        assert_eq!(d.mean, None);
    }

    #[test]
    fn nearest_rank_preserves_half_second_outlier() {
        let mut h = Histogram::default();
        for _ in 0..999 {
            h.record(10_000);
        }
        h.record(500_000);
        let d = h.summary();
        assert_eq!(d.p99, Some(10_000));
        assert_eq!(d.max, Some(500_000));
        assert_eq!(d.mean, Some(10_490.0));
    }
}
