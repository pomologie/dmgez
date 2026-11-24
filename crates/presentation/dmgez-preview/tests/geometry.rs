use dmgez_preview::{
	CanvasPoint, Drag, FinderScreenPoint, FolderPoint, FolderRect, GeometryError, Grid, Size,
	Viewport, window_top_left,
};
use proptest::prelude::*;

#[test]
fn window_origin_flips_y_before_converting_to_top_left() {
	let lower_left = FinderScreenPoint::new(30.0, 40.0).unwrap();
	let size = Size::new(640.0, 280.0).unwrap();
	let screen = CanvasPoint::new(100.0, 1080.0).unwrap();
	let result = window_top_left(lower_left, size, screen).unwrap();
	assert_eq!(result, CanvasPoint::new(130.0, 760.0).unwrap());
}

#[test]
fn invalid_and_overflowing_geometry_returns_typed_errors() {
	assert_eq!(
		FolderPoint::new(f64::NAN, 0.0),
		Err(GeometryError::NonFinite)
	);
	assert_eq!(Size::new(0.0, 1.0), Err(GeometryError::Size));
	let zero_canvas = CanvasPoint::new(0.0, 0.0).unwrap();
	let zero_folder = FolderPoint::new(0.0, 0.0).unwrap();
	for scale in [-1.0, 0.0, f64::INFINITY, f64::NAN, f64::from_bits(1)] {
		assert_eq!(
			Viewport::new(zero_canvas, zero_folder, scale),
			Err(GeometryError::Scale)
		);
	}
	let viewport = Viewport::new(zero_canvas, zero_folder, 2.0).unwrap();
	let huge = FolderPoint::new(f64::MAX, 0.0).unwrap();
	assert_eq!(viewport.to_canvas(huge), Err(GeometryError::NonFinite));
	assert_eq!(
		FolderRect::centered(huge, Size::new(16.0, 16.0).unwrap()),
		Err(GeometryError::Rectangle)
	);
	assert_eq!(
		Grid::new(zero_folder, -1.0),
		Err(GeometryError::GridSpacing)
	);
}

#[test]
fn rectangles_are_half_open_and_touching_edges_are_not_overlap() {
	let bounds = FolderRect::from_top_left(
		FolderPoint::new(4.0, 5.0).unwrap(),
		Size::new(16.0, 12.0).unwrap(),
	)
	.unwrap();
	let adjacent = FolderRect::from_top_left(bounds.max(), Size::new(1.0, 1.0).unwrap()).unwrap();
	assert!(bounds.contains(bounds.min()));
	assert!(!bounds.contains(bounds.max()));
	assert!(bounds.contains_rect(bounds));
	assert!(!bounds.contains_rect(adjacent));
	assert!(!bounds.intersects(adjacent));
}

#[test]
fn drag_preserves_grab_offset_during_pan_and_optional_snap() {
	let viewport = Viewport::new(
		CanvasPoint::new(2.0, 7.0).unwrap(),
		FolderPoint::new(10.0, 20.0).unwrap(),
		2.0,
	)
	.unwrap();
	let pointer = viewport
		.to_canvas(FolderPoint::new(35.0, 45.0).unwrap())
		.unwrap();
	let center = FolderPoint::new(40.0, 50.0).unwrap();
	let drag = Drag::begin(center, pointer, viewport).unwrap();
	assert_eq!(drag.center_at(pointer, viewport, None).unwrap(), center);

	let panned = Viewport::new(
		CanvasPoint::new(2.0, 9.0).unwrap(),
		FolderPoint::new(12.0, 3.0).unwrap(),
		1.25,
	)
	.unwrap();
	let next_pointer = panned
		.to_canvas(FolderPoint::new(53.0, 66.0).unwrap())
		.unwrap();
	assert_eq!(
		drag.center_at(next_pointer, panned, None).unwrap(),
		FolderPoint::new(58.0, 71.0).unwrap()
	);
	let grid = Grid::new(FolderPoint::new(4.0, 4.0).unwrap(), 16.0).unwrap();
	assert_eq!(
		drag.center_at(next_pointer, panned, Some(grid)).unwrap(),
		FolderPoint::new(52.0, 68.0).unwrap()
	);
}

proptest! {
	#[test]
	fn scaled_scrolled_coordinates_round_trip(
		(x, y) in (-8192.0_f64 .. 8192.0, -8192.0_f64 .. 8192.0),
		(origin_x, origin_y) in (-4096.0_f64 .. 4096.0, -4096.0_f64 .. 4096.0),
		(scroll_x, scroll_y) in (-2048.0_f64 .. 2048.0, -2048.0_f64 .. 2048.0),
		scale in 0.125_f64 .. 16.0,
	) {
		let origin = CanvasPoint::new(origin_x, origin_y).unwrap();
		let scroll = FolderPoint::new(scroll_x, scroll_y).unwrap();
		let viewport = Viewport::new(origin, scroll, scale).unwrap();
		let logical = FolderPoint::new(x, y).unwrap();
		let canvas = viewport.to_canvas(logical).unwrap();
		let round_trip = viewport.to_folder(canvas).unwrap();
		const MAX_ROUND_TRIP_ERROR: f64 = 1e-10;
		prop_assert!((round_trip.x() - x).abs() < MAX_ROUND_TRIP_ERROR);
		prop_assert!((round_trip.y() - y).abs() < MAX_ROUND_TRIP_ERROR);
	}
}
