use dmgez_builder::{
	manifest::{Entry, EntryKind, SourceIdentity, SourceSnapshot},
	metadata::{Attribute, policy::HardLinks},
	planning::PreparedBuild,
};
use dmgez_config::VirtualPath;
use dmgez_udif::writer::Cancellation;
use hfsplus_builder::volume::{EntryKind as VolumeEntryKind, HardLinkGroup};

use super::{
	Error, MetadataLoss, Payload, charge_many, check_cancelled,
	metadata::{Capture, validate_finder_overrides},
	relative,
};

pub(super) struct Member<'build> {
	pub(super) identity: SourceIdentity,
	pub(super) entry: &'build Entry,
}

impl<'build> Member<'build> {
	pub(super) fn new(entry: &'build Entry, links: &HardLinks) -> Option<Self> {
		if !matches!(entry.kind, EntryKind::File) {
			return None;
		}
		Some(Self {
			identity: links.group_for(entry)?,
			entry,
		})
	}
}

pub(super) fn sort_members(members: &mut [Member<'_>]) {
	members.sort_unstable_by(|left, right| {
		left.identity.cmp(&right.identity).then_with(|| {
			left.entry
				.destination
				.as_str()
				.cmp(right.entry.destination.as_str())
		})
	});
}

pub(super) fn assemble<'build>(
	mut members: Vec<Member<'build>>,
	capture: &mut Capture<'_, '_>,
	pending_attrs: &mut Vec<(&'build str, Vec<Attribute>)>,
	losses: &mut Vec<MetadataLoss<'build>>,
	generated_seconds: u32,
	cancellation: &Cancellation,
	prepared: &PreparedBuild,
) -> Result<Vec<HardLinkGroup<'build, Payload<'build>>>, Error> {
	sort_members(&mut members);
	let groups = members.chunk_by(|left, right| left.identity == right.identity);
	let count = groups.clone().count();
	charge_many::<HardLinkGroup<'_, Payload<'_>>>(capture.remaining, count)?;
	let mut output = Vec::new();
	output.try_reserve_exact(count)?;
	for aliases in groups {
		let representative = aliases[0].entry;
		charge_many::<&str>(capture.remaining, aliases.len())?;
		let mut paths = Vec::new();
		paths.try_reserve_exact(aliases.len())?;
		for member in aliases {
			check_cancelled(cancellation)?;
			let alias = member.entry;
			validate_alias(representative, alias)?;
			paths.push(relative(&alias.destination)?);
		}

		let captured = capture.entry(representative, generated_seconds, prepared)?;
		if !captured.losses.is_empty() {
			let others = &aliases[1..];
			charge_many::<&VirtualPath>(capture.remaining, others.len())?;
			let mut other_paths = Vec::new();
			other_paths.try_reserve_exact(others.len())?;
			other_paths.extend(others.iter().map(|member| &member.entry.destination));
			losses.push(MetadataLoss {
				path: &representative.destination,
				aliases: other_paths,
				details: captured.losses,
			});
		}
		let entry = captured.entry;
		if !captured.attributes.is_empty() {
			// The volume draft resolves this visible path to the shared inode.
			pending_attrs.push((entry.path, captured.attributes));
		}
		let VolumeEntryKind::File { data, resource } = entry.kind else {
			return Err(unsupported(representative));
		};
		output.push(HardLinkGroup {
			paths,
			metadata: entry.metadata,
			data,
			resource,
		});
	}
	Ok(output)
}

pub(super) fn validate_alias(representative: &Entry, alias: &Entry) -> Result<(), Error> {
	validate_finder_overrides(alias)?;
	let original = source(representative)?.metadata();
	let alias_source = source(alias)?;
	let mut metadata = *alias_source.metadata();
	// Reads can change access time without replacing or modifying an inode.
	metadata.accessed = original.accessed;
	if &metadata != original {
		return Err(Error::LinkMetadata {
			path: alias.destination.as_str().into(),
		});
	}
	alias_source.validate().map_err(Error::LinkSource)
}

fn source(entry: &Entry) -> Result<&SourceSnapshot, Error> {
	entry.source.as_ref().ok_or_else(|| unsupported(entry))
}

fn unsupported(entry: &Entry) -> Error {
	Error::UnsupportedEntry {
		path: entry.destination.as_str().into(),
	}
}
