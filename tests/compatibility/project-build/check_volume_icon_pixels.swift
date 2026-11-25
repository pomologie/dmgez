// Check iconutil's slots and compare each isolated ICNS representation in ImageIO.
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

require(CommandLine.arguments.count == 4, "expected input, decoded, and isolated iconset directories")
let inputs = URL(fileURLWithPath: CommandLine.arguments[1])
let decoded = URL(fileURLWithPath: CommandLine.arguments[2])
let isolated = URL(fileURLWithPath: CommandLine.arguments[3])
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
