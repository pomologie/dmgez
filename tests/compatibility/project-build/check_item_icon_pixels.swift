// Compare Finder icon readback through NSWorkspace with the ICNS artwork.
import AppKit
import Foundation

let sampledSides = [16, 32, 128]
let channels = 4
let bitsPerChannel = 8

func require(_ condition: @autoclosure () -> Bool, _ message: String) {
	guard condition() else {
		FileHandle.standardError.write(Data(("Item icon: " + message + "\n").utf8))
		exit(EXIT_FAILURE)
	}
}

func pixels(_ image: NSImage, side: Int) -> Data {
	var rect = CGRect(x: 0, y: 0, width: side, height: side)
	guard let raster = image.cgImage(forProposedRect: &rect, context: nil, hints: nil),
		let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)
	else { fatalError("could not read icon pixels") }
	let rowBytes = side * channels
	let alpha = CGImageAlphaInfo.premultipliedLast.rawValue
	var bytes = Data(count: rowBytes * side)
	bytes.withUnsafeMutableBytes { raw in
		guard let context = CGContext(data: raw.baseAddress, width: side, height: side,
			bitsPerComponent: bitsPerChannel, bytesPerRow: rowBytes, space: colorSpace, bitmapInfo: alpha)
		else { fatalError("could not create icon raster context") }
		context.setBlendMode(.copy)
		context.draw(raster, in: rect)
	}
	return bytes
}

let args = CommandLine.arguments
require(args.count == 6, "expected ICNS, custom file and folder, and generic file and folder")
guard let fixture = NSImage(contentsOfFile: args[1])
else { fatalError("could not load the source ICNS") }

let targets = args[2...3].map { NSWorkspace.shared.icon(forFile: $0) }
let controls = args[4...5].map { NSWorkspace.shared.icon(forFile: $0) }
for side in sampledSides {
	let expected = pixels(fixture, side: side)
	for (target, path) in zip(targets, args[2...3]) {
		require(pixels(target, side: side) == expected, "incorrect \(side)-pixel icon at \(path)")
	}
	for (control, path) in zip(controls, args[4...5]) {
		require(pixels(control, side: side) != expected, "generic \(side)-pixel icon matched at \(path)")
	}
}
print("NSWorkspace returned source-identical icons for the mounted file and folder at 16, 32, and 128 pixels.")
