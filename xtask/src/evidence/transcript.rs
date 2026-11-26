use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use super::Result;

const KANI_HARNESS: &str = "Checking harness ";
const KANI_SUCCESS: &str = "VERIFICATION:- SUCCESSFUL";
const KANI_SUMMARY: &str = "Manual Harness Summary:";
const TEST_RESULT: &str = "test result: ";
const TEST_SUCCESS: &str = "ok. ";
const FUZZ_COMPLETE: &str = "Done ";

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Check {
	Kani {
		filter: String,
		harnesses: BTreeMap<String, usize>,
	},
	RustTests {
		minimum: usize,
	},
	Fuzz {
		minimum: usize,
	},
	Native {
		markers: BTreeMap<String, usize>,
	},
}

impl Check {
	pub(super) fn verify(&self, log: &str) -> Result<()> {
		match self {
			Self::Kani { harnesses, .. } => verify_kani(log, harnesses),
			Self::RustTests { minimum } => verify_tests(log, *minimum),
			Self::Fuzz { minimum } => {
				let count = log.lines().find_map(|line| {
					let (count, _) = line.strip_prefix(FUZZ_COMPLETE)?.split_once(" runs in ")?;
					count.parse::<usize>().ok()
				});
				if *minimum == 0
					|| !count.is_some_and(|count| count >= *minimum)
					|| !log.lines().any(|line| line == "No crash artifacts")
				{
					return Err("fuzz replay or crash-artifact check is incomplete".into());
				}
				Ok(())
			}
			Self::Native { markers } => {
				if markers.is_empty()
					|| markers.iter().any(|(text, count)| {
						text.is_empty() || *count == 0 || log.matches(text).count() < *count
					}) {
					return Err("native transcript is missing required oracle results".into());
				}
				Ok(())
			}
		}
	}
}

fn verify_kani(log: &str, expected: &BTreeMap<String, usize>) -> Result<()> {
	let (results, footer) = log.split_once(KANI_SUMMARY).ok_or("missing Kani summary")?;
	let summary = format!(
		"Complete - {} successfully verified harnesses, 0 failures, {} total.",
		expected.len(),
		expected.len()
	);
	if expected.is_empty() || !footer.lines().any(|line| line == summary) {
		return Err("Kani did not report all required harnesses successful".into());
	}
	let mut seen = BTreeSet::new();
	for block in results.split(KANI_HARNESS).skip(1) {
		let (id, block) = block
			.split_once('\n')
			.ok_or("invalid Kani harness result")?;
		let id = id.trim().trim_end_matches("...");
		let id = id.rsplit("::").next().ok_or("missing Kani harness name")?;
		let minimum = expected
			.get(id)
			.ok_or_else(|| format!("unexpected Kani harness {id}"))?;
		if !seen.insert(id)
			|| block.lines().filter(|line| *line == KANI_SUCCESS).count() != 1
			|| !verified_properties(block)
			|| !reached_covers(block, *minimum)
		{
			return Err(format!("incomplete or vacuous Kani result for {id}").into());
		}
	}
	if seen.len() != expected.len() {
		return Err("missing required Kani harness results".into());
	}
	Ok(())
}

fn verified_properties(log: &str) -> bool {
	log.lines().any(|line| {
		let mut words = line.split_whitespace();
		let (Some("**"), Some("0"), Some("of"), Some(total), Some("failed")) = (
			words.next(),
			words.next(),
			words.next(),
			words.next(),
			words.next(),
		) else {
			return false;
		};
		let unreachable = match (words.next(), words.next()) {
			(None, None) => Some(0),
			(Some(count), Some("unreachable)")) => count
				.strip_prefix('(')
				.and_then(|count| count.parse::<usize>().ok()),
			_ => None,
		};
		total
			.parse::<usize>()
			.ok()
			.zip(unreachable)
			.is_some_and(|(total, unreachable)| total > unreachable)
	})
}

fn reached_covers(log: &str, minimum: usize) -> bool {
	let mut total = 0usize;
	for line in log
		.lines()
		.filter(|line| line.contains("cover properties satisfied"))
	{
		let Some(count) = line
			.trim()
			.strip_prefix("** ")
			.and_then(|line| line.strip_suffix(" cover properties satisfied"))
			.and_then(|line| line.split_once(" of "))
			.and_then(|(actual, expected)| {
				(actual == expected).then(|| actual.parse::<usize>().ok())
			})
			.flatten()
		else {
			return false;
		};
		let Some(sum) = total.checked_add(count) else {
			return false;
		};
		total = sum;
	}
	total >= minimum
}

fn verify_tests(log: &str, minimum: usize) -> Result<()> {
	let mut total = 0usize;
	for result in log
		.lines()
		.filter_map(|line| line.strip_prefix(TEST_RESULT))
	{
		let (count, rest) = result
			.strip_prefix(TEST_SUCCESS)
			.and_then(|value| value.split_once(" passed; "))
			.ok_or("test transcript contains a failed suite")?;
		if !rest.starts_with("0 failed;") {
			return Err("test transcript contains failures".into());
		}
		total = total
			.checked_add(count.parse()?)
			.ok_or("test count overflow")?;
	}
	if minimum == 0 || total < minimum {
		return Err("test transcript has too few passing tests".into());
	}
	Ok(())
}
