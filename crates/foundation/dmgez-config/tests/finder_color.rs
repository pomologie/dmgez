//! Finder background color cases for D-FINDER-02.

use dmgez_config::{BackgroundColorError, parse_solid_background_color};

const TOLERANCE: f64 = 1.0e-12;

fn assert_color(value: &str, expected: [f64; 3]) {
	let (red, green, blue) = parse_solid_background_color(value).unwrap();
	for (actual, expected) in [red, green, blue].into_iter().zip(expected) {
		assert!(
			(0.0..=1.0).contains(&actual) && (actual - expected).abs() <= TOLERANCE,
			"{value}: expected {expected:?}, got {actual:?}",
		);
	}
}

#[test]
fn recognizes_named_short_hex_and_full_hex_colors() {
	for (value, expected) in [
		(" GoldenRod ", [218, 165, 32]),
		("grey", [128, 128, 128]),
		("gray", [128, 128, 128]),
		("RebeccaPurple", [102, 51, 153]),
		("yellowgreen", [154, 205, 50]),
		("#3aF", [51, 170, 255]),
		("#33AAff", [51, 170, 255]),
	] {
		assert_color(value, expected.map(|channel| f64::from(channel) / 255.0));
	}
}

#[test]
fn converts_fractional_rgb_cmyk_and_gray() {
	for (value, expected) in [
		("rgb(1, .25, 50 %)", [1.0, 0.25, 0.5]),
		("RGB(10%, .2, 3e-1)", [0.1, 0.2, 0.3]),
		("cmyk(.85, 25%, .1, .4)", [0.0, 0.35, 0.5]),
		("gray(30%)", [0.3; 3]),
		("grey(.425)", [0.425; 3]),
	] {
		assert_color(value, expected);
	}
}

#[test]
fn hsl_and_hwb_convert_hue_units_and_neutral_components() {
	for (value, expected) in [
		("hsl(-30, 100%, .5)", [1.0, 0.0, 0.5]),
		("hsl(480deg, 1, .5)", [0.0, 1.0, 0.0]),
		("HSL(4.1887902047863905 RAD, 100%, 50%)", [0.0, 0.0, 1.0]),
		("hsl(200grad, 1, .5)", [0.0, 1.0, 1.0]),
		("hsl(100gon, 1, .5)", [0.5, 1.0, 0.0]),
		("hsl(250, 0, .3)", [0.3; 3]),
		("hwb(300, .15, .25)", [0.75, 0.15, 0.75]),
		("hwb(120, 80%, 40%)", [2.0 / 3.0; 3]),
		("hwb(120, 25%, 75%)", [0.25; 3]),
	] {
		assert_color(value, expected);
	}
}

#[test]
fn rejects_alpha_out_of_range_values_and_incomplete_syntax() {
	for invalid in [
		"transparent",
		"currentcolor",
		"#abcd",
		"#ffcc007f",
		"#1+2000",
		"rgba(1, 0, 0, 1)",
		"rgb(1, 0, 0, 0.5)",
		"rgb(1 0 0 / .5)",
		"rgb(255, 0, 0)",
		"rgb(101%, 0, 0)",
		"rgb(1, -.1, 0)",
		"rgb(1, NaN, 0)",
		"rgb(1, inf, 0)",
		"rgb(1, 0)",
		"rgb(1, 0, 0,)",
		"rgb(1, 0, 0",
		"rgb(1, 0, 0) blue",
		"hsl(180%, .4, .5)",
		"hsl(1e400rad, .4, .5)",
		"hwb(120, 1.1, 0)",
		"cmyk(.1, .2, .3, 150%)",
		"grey()",
		"var(--color)",
	] {
		assert_eq!(
			parse_solid_background_color(invalid),
			Err(BackgroundColorError),
			"{invalid}",
		);
	}
}
