// Check iconutil's slots and compare each isolated ICNS representation in ImageIO.
import AppKit
import CoreGraphics
import Foundation
import ImageIO

let expectedRepresentationCount = 10
let bitsPerChannel = 8
let channels = 4

func require(_ condition: @autoclosure () -> Bool, _ message: String) {
	guard condition() else {
		FileHandle.standardError.write(Data(("Volume icon: " + message + "\n").utf8))
		exit(EXIT_FAILURE)
	}
}

func pngs(in directory: URL) throws -> [String] {
	try FileManager.default.contentsOfDirectory(atPath: directory.path)
		.filter { $0.hasSuffix(".png") }.sorted()
}

func decode(_ url: URL) -> CGImage {
	guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
		let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
	else { fatalError("ImageIO could not decode \(url.lastPathComponent)") }
	return image
}

func pixels(_ image: CGImage) -> Data {
	let rowBytes = image.width * channels
	let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
	let alpha = CGImageAlphaInfo.premultipliedLast.rawValue
	var bytes = Data(count: rowBytes * image.height)
	bytes.withUnsafeMutableBytes { raw in
		let context = CGContext(data: raw.baseAddress, width: image.width, height: image.height,
			bitsPerComponent: bitsPerChannel, bytesPerRow: rowBytes, space: colorSpace, bitmapInfo: alpha)!
		context.setBlendMode(.copy)
		context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
	}
	return bytes
}

func checkInputRepresentations(inputs: URL, decoded: URL, isolated: URL) throws {
	let originals = try pngs(in: inputs)
	let extracted = try pngs(in: decoded)
	require(originals.count == expectedRepresentationCount, "expected ten input representations")
	require(extracted == originals, "iconutil extracted a different set of named slot representations")

	for name in originals {
		let source = decode(inputs.appendingPathComponent(name))
		let result = decode(decoded.appendingPathComponent(name))
		let elementName = URL(fileURLWithPath: name).deletingPathExtension().lastPathComponent + ".icns"
		let element = decode(isolated.appendingPathComponent(elementName))
		require(source.width == result.width && source.height == result.height,
			"unexpected dimensions for \(name)")
		require(source.width == element.width && source.height == element.height,
			"unexpected isolated dimensions for \(name)")
		require(pixels(source) == pixels(element), "pixel or alpha mismatch for \(name)")
	}
	print("Iconutil found all ten slots. ImageIO matched the pixels and alpha of each isolated element.")
}

func checkArtwork(background: URL, isolated: URL) throws {
	let canvasWidth = 640
	let canvasHeight = 280
	let canvasPoints = CGSize(width: canvasWidth, height: canvasHeight)
	let standardDPI = 72
	let scales = [1, 2]
	let paper: [UInt8] = [246, 248, 251, 255]
	let arrow: [UInt8] = [30, 99, 174, 255]
	let badge: [UInt8] = [221, 28, 118, 255]
	let source = CGImageSourceCreateWithURL(background as CFURL, nil)!
	let artwork = NSImage(contentsOf: background)!
	require(CGImageSourceGetCount(source) == scales.count, "expected two background representations")
	require(artwork.representations.count == scales.count && artwork.size == canvasPoints,
		"unexpected AppKit background size or representation count")
	for (index, scale) in scales.enumerated() {
		let width = canvasWidth * scale
		let height = canvasHeight * scale
		let rep = artwork.representations[index]
		require(rep.pixelsWide == width && rep.pixelsHigh == height && rep.size == canvasPoints,
			"unexpected AppKit representation at scale \(scale)")
		let page = CGImageSourceCreateImageAtIndex(source, index, nil)!
		require(page.width == width && page.height == height, "unexpected built-in artwork pixel size")
		let density = CGImageSourceCopyPropertiesAtIndex(source, index, nil)! as NSDictionary
		for axis in [kCGImagePropertyDPIWidth, kCGImagePropertyDPIHeight] {
			require(density[axis] as? Int == standardDPI * scale, "unexpected artwork density")
		}
		let raster = pixels(page)
		let center = ((height / 2) * width + width / 2) * channels
		require(raster.prefix(channels).elementsEqual(paper), "unexpected background canvas color")
		require(raster[center..<(center + channels)].elementsEqual(arrow), "missing central blue arrow")
	}

	let icons = try FileManager.default.contentsOfDirectory(at: isolated, includingPropertiesForKeys: nil)
		.filter { $0.pathExtension == "icns" }
	require(icons.count == expectedRepresentationCount, "expected ten badged icon elements")
	for url in icons {
		let icon = pixels(decode(url))
		let offsets = stride(from: 0, to: icon.count, by: channels)
		require(icon[channels - 1] == 0, "expected a transparent icon corner in \(url.lastPathComponent)")
		require(offsets.contains { offset in icon[offset..<(offset + channels)].elementsEqual(badge) },
			"missing opaque badge pixels in \(url.lastPathComponent)")
		require(offsets.contains { offset in
			icon[offset + channels - 1] == 255 && !icon[offset..<(offset + channels)].elementsEqual(badge)
		}, "missing base artwork in \(url.lastPathComponent)")
	}
	print("AppKit and ImageIO verified both arrow scales and the pixels of all ten badged icon slots.")
}

require(CommandLine.arguments.count == 4, "expected iconset directories or --artwork with a background and icon directory")
let args = CommandLine.arguments
if args[1] == "--artwork" {
	try checkArtwork(background: URL(fileURLWithPath: args[2]), isolated: URL(fileURLWithPath: args[3]))
} else {
	try checkInputRepresentations(inputs: URL(fileURLWithPath: args[1]), decoded: URL(fileURLWithPath: args[2]),
		isolated: URL(fileURLWithPath: args[3]))
}
