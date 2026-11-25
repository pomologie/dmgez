use super::*;

const EXECUTABLE_NAME: &str = "Installer";
const RESOLVED_EXECUTABLE_NAME: &str = "Setup & Installer";
const MINIMAL_XML: &[u8] = br#"<plist version="1.0"><dict><key>CFBundleExecutable</key><string>Installer</string></dict></plist>"#;

#[test]
fn accepts_native_xml_and_binary_property_lists() {
	let document = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
	<key>Other</key><dict><key>CFBundleExecutable</key><false/></dict>
	<key>CFBundleExecutabl&#101;</key><string>Setup &amp; Installer</string>
</dict></plist>"#;
	let executable = executable(document, &mut || false).unwrap().unwrap();
	assert_eq!(executable, RESOLVED_EXECUTABLE_NAME);

	let mut fields = plist::Dictionary::new();
	fields.insert(BUNDLE_EXECUTABLE_KEY.to_owned(), EXECUTABLE_NAME.into());
	let mut binary = Vec::new();
	plist::Value::Dictionary(fields)
		.to_writer_binary(&mut binary)
		.unwrap();
	assert_eq!(
		super::executable(&binary, &mut || false).unwrap().unwrap(),
		EXECUTABLE_NAME
	);
}

#[test]
fn preserves_mixed_xml_text_and_cdata() {
	let document = r#"<plist><dict>
	<key>CFBundle<![CDATA[Executable]]></key>
	<string>Setup <![CDATA[<α & β>]]> &amp; <![CDATA[Installer]]></string>
</dict></plist>"#;
	assert_eq!(
		executable(document.as_bytes(), &mut || false)
			.unwrap()
			.unwrap(),
		"Setup <α & β> & Installer",
	);
}

#[test]
fn preserves_cdata_after_a_utf8_bom() {
	const UTF8_BOM: &[u8] = &[0xef, 0xbb, 0xbf];
	let document = [
		UTF8_BOM,
		br#"<?xml version="1.0" encoding="UTF-8"?>
<plist><dict><key>CFBundle<![CDATA[Exec]]><![CDATA[utable]]></key>
<string>Set<![CDATA[up & ]]><![CDATA[Install]]>er</string></dict></plist>"#,
	]
	.concat();
	assert_eq!(
		executable(&document, &mut || false).unwrap().unwrap(),
		RESOLVED_EXECUTABLE_NAME,
	);
	let repeated_bom = [UTF8_BOM, &document].concat();
	assert!(matches!(
		executable(&repeated_bom, &mut || false).unwrap(),
		Err(BundleInfoProblem::InvalidPlist),
	));
}

#[test]
fn charges_repeated_binary_data_references() {
	use plist::stream::{BinaryWriter, Writer};

	const CHUNK_BYTES: usize = 64 << 10;
	const REFERENCES: u64 = MAX_EXPANDED_BYTES / CHUNK_BYTES as u64;
	let data = vec![0; CHUNK_BYTES];
	let mut binary = Vec::new();
	let mut writer = BinaryWriter::new(&mut binary);
	writer.write_start_dictionary(Some(1)).unwrap();
	writer.write_string("AdditionalData".into()).unwrap();
	writer.write_start_array(Some(REFERENCES)).unwrap();
	for _ in 0..REFERENCES {
		writer.write_data(Cow::Borrowed(&data)).unwrap();
	}
	writer.write_end_collection().unwrap();
	writer.write_end_collection().unwrap();
	assert!(binary.len() < 2 * CHUNK_BYTES);
	assert!(matches!(
		executable(&binary, &mut || false),
		Err(Error::Limit {
			budget: Budget::PlistExpandedBytes,
			limit: MAX_EXPANDED_BYTES,
		}),
	));
}

#[test]
fn rejects_shape_ambiguities_and_external_entity_spelling() {
	for document in [
		b"<plist><dict><string>CFBundleExecutable</string><string>Installer</string></dict></plist>".as_slice(),
		b"<plist><dict><key>CFBundleExecutable</key><string>Installer</string><key>CFBundleExecutabl&#101;</key><string>Other</string></dict></plist>",
		b"<plist><dict><key>CFBundleExecutable</key><string>Installer&other;</string></dict></plist>",
		b"<!DOCTYPE plist [<!ENTITY other 'ignored'>]><plist><dict><key>CFBundleExecutable</key><string>Installer</string></dict></plist>",
		b"<plist><dict><key>CFBundleExecutable</key><string>Installer</string></dict></plist><dict/>",
		b"<plist><dict><key>Other</key><string>unexpected\0byte</string><key>CFBundleExecutable</key><string>Installer</string></dict></plist>",
	] {
		assert!(matches!(executable(document, &mut || false), Ok(Err(BundleInfoProblem::InvalidPlist))), "{document:?}");
	}
}

#[test]
fn rejects_executable_names_with_path_semantics() {
	for unsafe_name in [
		"../Installer",
		"/Installer",
		"C:Installer",
		"\\Installer",
		"..",
		"",
	] {
		let document = format!(
			"<dict><key>{BUNDLE_EXECUTABLE_KEY}</key><string>{unsafe_name}</string></dict>"
		);
		assert!(matches!(
			executable(document.as_bytes(), &mut || false),
			Ok(Err(BundleInfoProblem::InvalidExecutable))
		));
	}
}

#[test]
fn polling_and_depth_limits_stop_expansion() {
	let levels = MAX_DEPTH as usize;
	let document = format!(
		"<dict><key>nested</key>{}{}</dict>",
		"<array>".repeat(levels),
		"</array>".repeat(levels)
	);
	assert!(matches!(
		executable(document.as_bytes(), &mut || false),
		Err(Error::Limit {
			budget: Budget::PlistDepth,
			..
		})
	));
	assert!(matches!(
		executable(MINIMAL_XML, &mut || true),
		Err(Error::Cancelled)
	));
}
