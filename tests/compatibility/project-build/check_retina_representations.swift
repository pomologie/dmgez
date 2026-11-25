// Independent macOS oracle for the image referenced by a mounted Finder alias.
import AppKit
import ImageIO
import UniformTypeIdentifiers

func require(_ condition: @autoclosure () -> Bool, _ message: String) {
	guard condition() else {
		FileHandle.standardError.write(Data(("Retina TIFF: " + message + "\n").utf8))
		exit(EXIT_FAILURE)
	}
}

func imageSource(_ path: String) -> CGImageSource {
	let url = URL(fileURLWithPath: path)
	guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else {
		fatalError("ImageIO could not open \(path)")
	}
	return source
}

func decodedImage(_ source: CGImageSource, at page: Int) -> CGImage {
	guard let image = CGImageSourceCreateImageAtIndex(source, page, nil) else {
		fatalError("ImageIO could not decode page \(page)")
	}
	return image
}

func srgbBytes(_ image: CGImage) -> [UInt8] {
	// Compare both inputs in one color space and alpha layout. This exercises
	// the actual ImageIO decoder without assuming its underlying row storage.
	let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
	let channels = 4
	let rowBytes = image.width * channels
	let bitmapInfo = CGImageAlphaInfo.premultipliedLast.rawValue
	var bytes = [UInt8](repeating: 0, count: rowBytes * image.height)
	bytes.withUnsafeMutableBytes { raw in
		let context = CGContext(data: raw.baseAddress, width: image.width, height: image.height,
			bitsPerComponent: 8, bytesPerRow: rowBytes, space: colorSpace, bitmapInfo: bitmapInfo)!
		context.setBlendMode(.copy)
		context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
	}
	return bytes
}

require(CommandLine.arguments.count == 4, "usage: swift check_retina_representations.swift background.tiff standard.png retina.png")
let result = CommandLine.arguments[1]
let originals = CommandLine.arguments[2...].map { decodedImage(imageSource($0), at: 0) }
let tiff = imageSource(result)
require(CGImageSourceGetType(tiff) as String? == UTType.tiff.identifier, "expected a TIFF")
require(CGImageSourceGetCount(tiff) == 2, "expected two independently decodable representations")

let image = NSImage(contentsOfFile: result)!
let points = CGSize(width: originals[0].width, height: originals[0].height)
require(image.size == points, "incorrect AppKit logical image size")
require(image.representations.count == originals.count, "incorrect AppKit representation count")

for (page, original) in originals.enumerated() {
	let scale = page + 1
	let density = 72 * scale
	let width = Int(points.width) * scale
	let height = Int(points.height) * scale
	let decoded = decodedImage(tiff, at: page)
	require(decoded.width == width && decoded.height == height, "incorrect pixel dimensions on page \(page)")
	let properties = CGImageSourceCopyPropertiesAtIndex(tiff, page, nil)! as NSDictionary
	for dpiKey in [kCGImagePropertyDPIWidth, kCGImagePropertyDPIHeight] {
		require(properties[dpiKey] as? Int == density, "incorrect DPI on page \(page)")
	}
	require(srgbBytes(decoded) == srgbBytes(original), "pixels differ from input on page \(page)")
	let rep = image.representations[page]
	require(rep.pixelsWide == width && rep.pixelsHigh == height, "incorrect AppKit pixel size")
	require(rep.size == points, "incorrect AppKit representation point size")

	// Give AppKit a drawing context of the same point size at each scale.
	let target = NSBitmapImageRep(bitmapDataPlanes: nil,
		pixelsWide: width, pixelsHigh: height, bitsPerSample: 8, samplesPerPixel: 4,
		hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
	target.size = points
	let context = NSGraphicsContext(bitmapImageRep: target)!
	require(context.cgContext.ctm.a == CGFloat(scale), "invalid backing scale for test context")
	let chosen = image.bestRepresentation(for: CGRect(origin: .zero, size: points), context: context, hints: nil)
	require(chosen?.pixelsWide == width && chosen?.pixelsHigh == height, "AppKit did not select the \(scale)x representation")
}

print("ImageIO pixels match both sources. AppKit selects the expected 1x and 2x representations.")
