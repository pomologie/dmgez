use std::{fmt, marker::PhantomData};

use serde::{Deserialize, Deserializer, Serialize, de};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(super) struct Input {
	pub title: String,
	pub contents: Vec<Object<InputEntry>>,
	#[serde(default, deserialize_with = "present")]
	pub icon: Option<String>,
	#[serde(default, deserialize_with = "present")]
	pub background: Option<String>,
	#[serde(default, deserialize_with = "present")]
	pub background_color: Option<String>,
	#[serde(default, deserialize_with = "present")]
	pub icon_size: Option<Integer>,
	#[serde(default, deserialize_with = "present")]
	pub window: Option<Object<Window>>,
	#[serde(default, deserialize_with = "present")]
	pub format: Option<Format>,
	#[serde(default, deserialize_with = "present")]
	pub filesystem: Option<Filesystem>,
	#[serde(default, deserialize_with = "present")]
	pub code_sign: Option<Object<Signing>>,
}

// A missing optional property is valid. A present property must match its
// declared type, including when its value is JSON null.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
	deserializer: D,
) -> Result<Option<T>, D::Error> {
	T::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Window {
	#[serde(default, deserialize_with = "present")]
	pub position: Option<Object<Point>>,
	#[serde(default, deserialize_with = "present")]
	pub size: Option<Object<Extent>>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Point {
	pub x: Integer,
	pub y: Integer,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Extent {
	pub width: Integer,
	pub height: Integer,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Signing {
	#[serde(rename = "signing-identity")]
	_signing_identity: String,
	#[serde(rename = "identifier", default, deserialize_with = "present")]
	_identifier: Option<String>,
}

#[derive(Clone, Copy, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all(deserialize = "UPPERCASE", serialize = "lowercase"))]
pub(super) enum Format {
	Udrw,
	Udro,
	Udco,
	#[default]
	Udzo,
	Udbz,
	Ulfo,
	Ulmo,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
pub(super) enum Filesystem {
	#[default]
	#[serde(rename(deserialize = "HFS+", serialize = "hfsplus"))]
	Hfsplus,
	#[serde(rename(deserialize = "APFS", serialize = "apfs"))]
	Apfs,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InputEntry {
	pub x: Integer,
	pub y: Integer,
	#[serde(rename = "type")]
	pub kind: EntryKind,
	pub path: String,
	#[serde(default, deserialize_with = "present")]
	pub name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum EntryKind {
	File,
	Link,
	Position,
}

// JSON numbers such as 80.0 or 8e1 count as integers in appdmg's schema.
#[derive(Clone, Copy, Deserialize)]
#[serde(try_from = "serde_json::Number")]
pub(super) struct Integer(pub i64);

impl TryFrom<serde_json::Number> for Integer {
	type Error = &'static str;

	fn try_from(number: serde_json::Number) -> Result<Self, Self::Error> {
		const MIN: f64 = i64::MIN as f64;
		const UPPER_BOUND: f64 = -MIN;
		if let Some(integer) = number.as_i64() {
			return Ok(Self(integer));
		}
		// The floating-point upper bound is exclusive because i64::MAX rounds
		// up to 2^63. Large JSON literals that exceed i64 are rejected.
		if number.is_f64()
			&& let Some(float) = number.as_f64()
			&& float.fract() == 0.0
			&& (MIN..UPPER_BOUND).contains(&float)
		{
			return Ok(Self(float as i64));
		}
		Err("expected an integer in the signed 64-bit range")
	}
}

// Serde's derived struct visitor also accepts positional sequences. Require a
// JSON object while letting the derived visitors reject unknown and duplicate keys.
#[derive(Clone, Copy)]
pub(super) struct Object<T>(pub T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		struct ObjectVisitor<T>(PhantomData<T>);

		impl<'de, T: Deserialize<'de>> de::Visitor<'de> for ObjectVisitor<T> {
			type Value = Object<T>;

			fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
				formatter.write_str("a JSON object")
			}

			fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
				T::deserialize(de::value::MapAccessDeserializer::new(map)).map(Object)
			}
		}

		deserializer.deserialize_map(ObjectVisitor(PhantomData))
	}
}
