use dmgez_operations::install::{CopyLimits, CopyPlanError, prepare_copy};

use super::*;

pub(super) const LINK_TARGET: &[u8] = b"MacOS/Installer";
pub(super) const RESOURCE_BYTES: &[u8] = b"resource payload";

pub(super) fn selection(
	view: &mut VolumeView<Fixture<'_>>,
) -> dmgez_operations::install::PlannedApplication<Node> {
	prepare_application(
		view,
		&SELECTED_APP.parse().unwrap(),
		QUALIFIED_PROFILE,
		NameLimits::default(),
		|| false,
	)
	.unwrap()
}

#[test]
fn refuses_escape_through_another_symlink_and_quarantined_objects() {
	let reads = Cell::new(0);
	for target in [
		b"/outside".as_slice(),
		b"../../outside",
		b"MacOS/../Contents",
	] {
		let mut view = Fixture {
			link: Some(target),
			..Fixture::new(&reads)
		}
		.view();
		let application = selection(&mut view);
		assert!(matches!(
			prepare_copy(&mut view, application, CopyLimits::default(), || false),
			Err(CopyPlanError::Link(Some(_)))
		));
	}
	let mut view = Fixture {
		link: Some(LINK_TARGET),
		quarantined: Some(Node::Executable),
		..Fixture::new(&reads)
	}
	.view();
	let application = selection(&mut view);
	assert!(matches!(
		prepare_copy(&mut view, application, CopyLimits::default(), || false),
		Err(CopyPlanError::Quarantine(Some(_)))
	));
}

#[test]
fn charges_payload_and_empty_attributes_before_opening_a_destination() {
	let reads = Cell::new(0);
	let mut view = Fixture {
		link: Some(LINK_TARGET),
		attribute: Some(b""),
		resource: Some(RESOURCE_BYTES),
		..Fixture::new(&reads)
	}
	.view();
	let application = selection(&mut view);
	let total =
		(INFO.len() + PROGRAM_BYTES.len() + LINK_TARGET.len() + RESOURCE_BYTES.len()) as u64;
	let limits = CopyLimits {
		max_payload_bytes: total,
		max_attributes: 2,
		..CopyLimits::default()
	};
	let plan = prepare_copy(&mut view, application, limits, || false).unwrap();
	assert_eq!(plan.summary().payload_bytes, total);
	assert_eq!(plan.summary().attributes, 2);
	assert_eq!(reads.get(), 1);
	let application = selection(&mut view);
	assert!(matches!(
		prepare_copy(
			&mut view,
			application,
			CopyLimits {
				max_payload_bytes: total - 1,
				..limits
			},
			|| false
		),
		Err(CopyPlanError::PayloadLimit)
	));
	let application = selection(&mut view);
	assert!(matches!(
		prepare_copy(
			&mut view,
			application,
			CopyLimits {
				max_attributes: 1,
				..limits
			},
			|| false
		),
		Err(CopyPlanError::AttributeCount)
	));
}

#[test]
fn admits_a_bounded_binary_info_plist() {
	let mut info = plist::Dictionary::new();
	info.insert("CFBundleExecutable".into(), EXECUTABLE_NAME.into());
	let mut bytes = Vec::new();
	plist::Value::Dictionary(info)
		.to_writer_binary(&mut bytes)
		.unwrap();
	let reads = Cell::new(0);
	let mut view = Fixture {
		info: Some(&bytes),
		..Fixture::new(&reads)
	}
	.view();
	let plan = prepare_application(
		&mut view,
		&SELECTED_APP.parse().unwrap(),
		QUALIFIED_PROFILE,
		NameLimits::default(),
		|| false,
	)
	.unwrap();
	assert_eq!(plan.executable().native_id(), Node::Executable);
	assert_eq!(reads.get(), 1);
}
