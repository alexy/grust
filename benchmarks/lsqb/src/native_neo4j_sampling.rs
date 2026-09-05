//! Explicit sampling schedule; warm-ups never become measurement samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Sampling {
    pub warmups: u32,
    pub runs: u32,
    pub legacy: bool,
}

impl Sampling {
    pub fn parse(args: &[String]) -> Result<Self, &'static str> {
        if args.is_empty() {
            return Ok(Self {
                warmups: 0,
                runs: 1,
                legacy: true,
            });
        }
        if args.len() != 2 {
            return Err("sampling requires WARMUPS RUNS");
        }
        let warmups = args[0].parse().map_err(|_| "invalid warmup count")?;
        let runs = args[1].parse().map_err(|_| "invalid measurement count")?;
        if warmups > 5 || !(1..=10).contains(&runs) {
            return Err("sampling permits 0-5 warmups and 1-10 measurements per query");
        }
        Ok(Self {
            warmups,
            runs,
            legacy: false,
        })
    }

    pub fn plan(self) -> Vec<(&'static str, u32)> {
        (0..self.warmups)
            .map(|i| ("warmup", i))
            .chain((0..self.runs).map(|i| ("measurement", i)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Sampling;

    #[test]
    fn explicit_schedule_separates_warmups_and_measurements() {
        let sample = Sampling::parse(&["2".into(), "3".into()]).unwrap();
        assert_eq!(
            sample.plan(),
            vec![
                ("warmup", 0),
                ("warmup", 1),
                ("measurement", 0),
                ("measurement", 1),
                ("measurement", 2)
            ]
        );
        assert!(!sample.legacy);
        assert!(Sampling::parse(&[]).unwrap().legacy);
    }

    #[test]
    fn invalid_or_unbounded_sampling_is_rejected() {
        for (warmups, runs) in [("0", "0"), ("6", "1"), ("0", "11"), ("-1", "1"), ("x", "1")] {
            assert!(Sampling::parse(&[warmups.into(), runs.into()]).is_err());
        }
        assert!(Sampling::parse(&["2".into()]).is_err());
    }
}
