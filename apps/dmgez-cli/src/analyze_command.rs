use std::{
	num::{NonZeroU32, NonZeroUsize},
	path::PathBuf,
};

use clap::Args;

#[cfg(feature = "inspect")]
use crate::inspect;
use crate::{CliResult, volume_commands::PARTITION_SLOT_VALUE_NAME};

#[cfg(feature = "hfsplus-analysis")]
mod hfsplus;

const HFSPLUS_ARG: &str = "hfsplus";
#[cfg(not(feature = "hfsplus-analysis"))]
const HFSPLUS_ANALYSIS_FEATURE: &str = "hfsplus-analysis";
#[cfg(not(feature = "inspect"))]
const INSPECT_FEATURE: &str = "inspect";

#[derive(Args)]
pub(super) struct Arguments {
	image: PathBuf,
	/// Write a versioned JSON report.
	#[arg(long)]
	json: bool,
	/// Check the selected HFS+ or HFSX bitmap and rank visible regular files.
	#[arg(long, id = HFSPLUS_ARG)]
	hfsplus: bool,
	/// Select a one-based partition slot. Otherwise one data window is required.
	#[arg(long, requires = HFSPLUS_ARG, value_name = PARTITION_SLOT_VALUE_NAME)]
	partition: Option<NonZeroU32>,
	/// Number of largest files to include.
	#[arg(long, requires = HFSPLUS_ARG)]
	largest: Option<NonZeroUsize>,
}

pub(super) fn run(arguments: &Arguments) -> CliResult {
	if arguments.hfsplus {
		#[cfg(feature = "hfsplus-analysis")]
		return hfsplus::run(arguments);
		#[cfg(not(feature = "hfsplus-analysis"))]
		return Err(format!("feature not compiled: {HFSPLUS_ANALYSIS_FEATURE}").into());
	}
	#[cfg(feature = "inspect")]
	{
		inspect::analyze(&arguments.image, arguments.json)
	}
	#[cfg(not(feature = "inspect"))]
	{
		Err(format!("feature not compiled: {INSPECT_FEATURE}").into())
	}
}
