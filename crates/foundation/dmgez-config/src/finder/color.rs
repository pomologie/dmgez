use std::f64::consts::TAU;

use thiserror::Error;

mod named;

const HEX_PREFIX: char = '#';
const HEX_RADIX: u8 = 16;
const NIBBLE_CHANNEL_SCALE: u8 = 17;
const PERCENT_SCALE: f64 = 100.0;
const DEGREES_PER_REVOLUTION: f64 = 360.0;
const GRADIANS_PER_REVOLUTION: f64 = 400.0;
const HUE_SECTORS: f64 = 6.0;
const MAX_FUNCTION_NAME_BYTES: usize = 4;

type Channels = [f64; 3];

/// A solid Finder background must describe an opaque color.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("Finder background color must be opaque, with a recognized syntax and components in range")]
pub struct BackgroundColorError;

/// Parses the Finder background color syntax into normalized sRGB channels.
///
/// Accepts opaque CSS color keywords, three- and six-digit hexadecimal RGB,
/// and the comma-separated `rgb`, `hsl`, `hwb`, `cmyk` and `gray`/`grey`
/// functions used by dmgbuild. Components use fractions or percentages. Hue
/// angles wrap and default to degrees, with explicit `deg`, `rad`, `grad`
/// and `gon` units also accepted.
///
/// # Errors
///
/// Returns [`BackgroundColorError`] for unsupported forms, including alpha
/// channels, and for malformed or out-of-range components.
pub fn parse_solid_background_color(input: &str) -> Result<(f64, f64, f64), BackgroundColorError> {
	let input = input.trim();
	let [red, green, blue] = if let Some(hex) = input.strip_prefix(HEX_PREFIX) {
		parse_hex(hex).map(normalize_bytes)
	} else if let Some((function, arguments)) = input.split_once('(') {
		arguments
			.strip_suffix(')')
			.and_then(|arguments| parse_function(function, arguments))
	} else {
		named::lookup(input).map(normalize_bytes)
	}
	.ok_or(BackgroundColorError)?;
	Ok((red, green, blue))
}

fn parse_hex(value: &str) -> Option<[u8; 3]> {
	let digit = |byte| u8::try_from(char::from(byte).to_digit(u32::from(HEX_RADIX))?).ok();
	let pair = |high, low| Some(digit(high)? * HEX_RADIX + digit(low)?);
	match *value.as_bytes() {
		[red, green, blue] => Some([
			digit(red)? * NIBBLE_CHANNEL_SCALE,
			digit(green)? * NIBBLE_CHANNEL_SCALE,
			digit(blue)? * NIBBLE_CHANNEL_SCALE,
		]),
		[
			red_high,
			red_low,
			green_high,
			green_low,
			blue_high,
			blue_low,
		] => Some([
			pair(red_high, red_low)?,
			pair(green_high, green_low)?,
			pair(blue_high, blue_low)?,
		]),
		_ => None,
	}
}

fn normalize_bytes(rgb: [u8; 3]) -> Channels {
	rgb.map(|channel| f64::from(channel) / f64::from(u8::MAX))
}

fn parse_function(function: &str, arguments: &str) -> Option<Channels> {
	let mut folded = [0; MAX_FUNCTION_NAME_BYTES];
	let name = folded.get_mut(..function.len())?;
	name.copy_from_slice(function.as_bytes());
	name.make_ascii_lowercase();
	match &*name {
		b"rgb" => {
			let [red, green, blue] = components(arguments)?;
			Some([fraction(red)?, fraction(green)?, fraction(blue)?])
		}
		b"hsl" => {
			let [hue, saturation, lightness] = components(arguments)?;
			Some(hsl(
				hue_degrees(hue)?,
				fraction(saturation)?,
				fraction(lightness)?,
			))
		}
		b"hwb" => {
			let [hue, white, black] = components(arguments)?;
			Some(hwb(hue_degrees(hue)?, fraction(white)?, fraction(black)?))
		}
		b"cmyk" => {
			let [cyan, magenta, yellow, black] = components(arguments)?;
			let black = fraction(black)?;
			let cmy = [fraction(cyan)?, fraction(magenta)?, fraction(yellow)?];
			// Match dmgbuild's additive ink approximation.
			Some(cmy.map(|ink| 1.0 - (ink + black).min(1.0)))
		}
		b"gray" | b"grey" => {
			let [gray] = components(arguments)?;
			Some([fraction(gray)?; 3])
		}
		_ => None,
	}
}

fn components<const COUNT: usize>(input: &str) -> Option<[&str; COUNT]> {
	let mut fields = input.split(',').map(str::trim);
	let mut components = [""; COUNT];
	for component in &mut components {
		*component = fields.next()?;
	}
	fields.next().is_none().then_some(components)
}

fn fraction(input: &str) -> Option<f64> {
	let (number, scale) = input
		.strip_suffix('%')
		.map_or((input, 1.0), |number| (number.trim_end(), PERCENT_SCALE));
	let value = number.parse::<f64>().ok()? / scale;
	(0.0..=1.0).contains(&value).then_some(value)
}

fn hue_degrees(input: &str) -> Option<f64> {
	let end = input
		.trim_end_matches(|character: char| character.is_ascii_alphabetic())
		.len();
	let (number, unit) = input.split_at(end);
	let angle = number.trim_end().parse::<f64>().ok()?;
	if !angle.is_finite() {
		return None;
	}
	if unit.is_empty() || unit.eq_ignore_ascii_case("deg") {
		Some(angle.rem_euclid(DEGREES_PER_REVOLUTION))
	} else if unit.eq_ignore_ascii_case("rad") {
		Some(angle.rem_euclid(TAU).to_degrees())
	} else if unit.eq_ignore_ascii_case("grad") || unit.eq_ignore_ascii_case("gon") {
		let degrees_per_gradian = DEGREES_PER_REVOLUTION / GRADIANS_PER_REVOLUTION;
		Some(angle.rem_euclid(GRADIANS_PER_REVOLUTION) * degrees_per_gradian)
	} else {
		None
	}
}

fn hue_rgb(degrees: f64) -> Channels {
	let sector = degrees / (DEGREES_PER_REVOLUTION / HUE_SECTORS);
	let half_circle = HUE_SECTORS / 2.0;
	// The three unit-chroma channel ramps are equally spaced around the hue wheel.
	[0.0, 4.0, 2.0].map(|offset| {
		let distance = ((sector + offset).rem_euclid(HUE_SECTORS) - half_circle).abs();
		(distance - 1.0).clamp(0.0, 1.0)
	})
}

fn hsl(hue: f64, saturation: f64, lightness: f64) -> Channels {
	let chroma = saturation * (1.0 - (2.0 * lightness - 1.0).abs());
	let dark = lightness - chroma / 2.0;
	hue_rgb(hue).map(|channel| (dark + chroma * channel).clamp(0.0, 1.0))
}

fn hwb(hue: f64, white: f64, black: f64) -> Channels {
	let neutral = white + black;
	// CSS Color 4 §8.1 defines an achromatic result once the neutral mix is full.
	if neutral >= 1.0 {
		return [white / neutral; 3];
	}
	hue_rgb(hue).map(|channel| (white + channel * (1.0 - neutral)).clamp(0.0, 1.0))
}
