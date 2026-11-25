use ds_parser::FieldKey;
use plist::{Dictionary, Value};

use super::{
	ASCENDING, COLUMNS, EXACT_INTEGER_LIMIT, ICON_SIZE, IDENTIFIER, INDEX, ListView, ReadError,
	ReadLimits, VISIBLE, WIDTH, read,
};

fn inspect(field: FieldKey, settings: Dictionary) -> Result<ListView, ReadError> {
	let mut bytes = Vec::new();
	plist::to_writer_binary(&mut bytes, &settings).unwrap();
	read(&bytes, ReadLimits::default(), field)
}

fn column() -> Dictionary {
	[
		(WIDTH, Value::Integer(200.into())),
		(VISIBLE, Value::Boolean(true)),
		(ASCENDING, Value::Boolean(false)),
	]
	.into_iter()
	.collect()
}

#[test]
fn refuses_ambiguous_column_identity_in_both_forms() {
	let mut first = column();
	first.insert(INDEX.into(), Value::Integer(0.into()));
	let keyed = [
		("name", Value::Dictionary(first.clone())),
		("size", Value::Dictionary(first)),
	]
	.into_iter()
	.collect();
	let settings = [(COLUMNS, Value::Dictionary(keyed))].into_iter().collect();
	assert!(matches!(
		inspect(FieldKey::lsvp, settings),
		Err(ReadError::PlistProfile)
	));

	let mut first = column();
	first.insert(IDENTIFIER.into(), Value::String("name".into()));
	let ordered = vec![Value::Dictionary(first.clone()), Value::Dictionary(first)];
	let settings = [(COLUMNS, Value::Array(ordered))].into_iter().collect();
	assert!(matches!(
		inspect(FieldKey::lsvC, settings),
		Err(ReadError::PlistProfile)
	));
}

#[test]
fn refuses_nonfinite_lossy_or_wrongly_typed_known_sizes() {
	let inspect_size = |value| inspect(FieldKey::lsvp, [(ICON_SIZE, value)].into_iter().collect());
	assert!(matches!(
		inspect_size(Value::Real(f64::NAN)),
		Err(ReadError::SettingsNumber)
	));
	assert!(matches!(
		inspect_size(Value::Integer((EXACT_INTEGER_LIMIT + 1).into())),
		Err(ReadError::PlistProfile)
	));
	assert!(matches!(
		inspect_size(Value::Boolean(true)),
		Err(ReadError::PlistProfile)
	));
	assert_eq!(
		inspect_size(Value::Integer(16.into())).unwrap().icon_size,
		Some(16.0)
	);
}
