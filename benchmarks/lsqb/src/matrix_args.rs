use std::env;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatrixArguments {
    pub backend: String,
    pub suite: String,
    pub scale: String,
    pub warmups: u32,
    pub runs: u32,
    pub query_timeout_ms: u64,
    pub cell_timeout_ms: u64,
    pub lsqb_root: PathBuf,
    pub attacks_dir: PathBuf,
    pub output: PathBuf,
}

impl MatrixArguments {
    pub fn parse() -> Result<Self, String> {
        Self::parse_from(env::args().skip(1))
    }

    fn parse_from(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut backend = None;
        let mut suite = "baseline".to_string();
        let mut scale = "example".to_string();
        let mut warmups = 2_u32;
        let mut runs = 10_u32;
        let mut query_timeout_ms = 30_000_u64;
        let mut cell_timeout_ms = None;
        let mut lsqb_root = PathBuf::from("/opt/lsqb");
        let mut attacks_dir = PathBuf::from("/opt/grust-attacks");
        let mut output = None;
        while let Some(flag) = args.next() {
            if matches!(flag.as_str(), "-h" | "--help") {
                return Err(usage().to_string());
            }
            let value = args
                .next()
                .ok_or_else(|| format!("{flag} requires a value\n\n{}", usage()))?;
            match flag.as_str() {
                "--backend" => backend = Some(value),
                "--suite" => suite = value,
                "--scale" => scale = value,
                "--warmups" => warmups = parse_positive_or_zero(&flag, &value)?,
                "--runs" => runs = parse_positive(&flag, &value)?,
                "--query-timeout-ms" => query_timeout_ms = parse_positive(&flag, &value)?,
                "--cell-timeout-ms" => cell_timeout_ms = Some(parse_positive(&flag, &value)?),
                "--lsqb-root" => lsqb_root = PathBuf::from(value),
                "--attacks-dir" => attacks_dir = PathBuf::from(value),
                "--output" => output = Some(PathBuf::from(value)),
                other => return Err(format!("unknown argument {other:?}\n\n{}", usage())),
            }
        }
        if !matches!(suite.as_str(), "baseline" | "adversarial") {
            return Err(format!(
                "unknown suite {suite:?}; use baseline or adversarial"
            ));
        }
        let backend = backend.ok_or_else(|| format!("--backend is required\n\n{}", usage()))?;
        let cell_timeout_ms = cell_timeout_ms
            .ok_or_else(|| format!("--cell-timeout-ms is required\n\n{}", usage()))?;
        let output = output.unwrap_or_else(|| {
            Path::new("out").join(format!("matrix-{suite}-{backend}-sf{scale}.json"))
        });
        Ok(Self {
            backend,
            suite,
            scale,
            warmups,
            runs,
            query_timeout_ms,
            cell_timeout_ms,
            lsqb_root,
            attacks_dir,
            output,
        })
    }
}

fn parse_positive<T>(flag: &str, value: &str) -> Result<T, String>
where
    T: std::str::FromStr + PartialOrd + Default,
{
    let parsed = value
        .parse::<T>()
        .map_err(|_| format!("invalid {flag} value {value:?}"))?;
    if parsed <= T::default() {
        return Err(format!("{flag} must be greater than zero"));
    }
    Ok(parsed)
}

fn parse_positive_or_zero<T>(flag: &str, value: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    value
        .parse::<T>()
        .map_err(|_| format!("invalid {flag} value {value:?}"))
}

pub fn usage() -> &'static str {
    "Usage: grust-lsqb-matrix --backend NAME [--suite baseline|adversarial] \
     [--scale SCALE] [--warmups N] [--runs N] [--query-timeout-ms MS] \
     --cell-timeout-ms MS \
     [--lsqb-root PATH] [--attacks-dir PATH] [--output PATH]"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(values: &[&str]) -> Result<MatrixArguments, String> {
        MatrixArguments::parse_from(values.iter().map(|value| (*value).to_string()))
    }

    #[test]
    fn parses_explicit_timing_protocol() {
        let args = parse(&[
            "--backend",
            "memory",
            "--scale",
            "0.1",
            "--warmups",
            "0",
            "--runs",
            "3",
            "--query-timeout-ms",
            "42",
            "--cell-timeout-ms",
            "420",
        ])
        .unwrap();
        assert_eq!(args.backend, "memory");
        assert_eq!(args.scale, "0.1");
        assert_eq!(args.warmups, 0);
        assert_eq!(args.runs, 3);
        assert_eq!(args.query_timeout_ms, 42);
        assert_eq!(args.cell_timeout_ms, 420);
    }

    #[test]
    fn requires_a_backend_and_measurement() {
        assert!(parse(&[]).unwrap_err().contains("--backend is required"));
        assert!(
            parse(&["--backend", "memory", "--runs", "0"])
                .unwrap_err()
                .contains("greater than zero")
        );
        assert!(
            parse(&["--backend", "memory"])
                .unwrap_err()
                .contains("--cell-timeout-ms is required")
        );
        assert!(
            parse(&["--backend", "memory", "--cell-timeout-ms", "0",])
                .unwrap_err()
                .contains("greater than zero")
        );
    }
}
