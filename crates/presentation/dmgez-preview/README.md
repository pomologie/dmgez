# Headless preview geometry

`ProjectFolder` combines a selected virtual folder's typed Finder settings with its resolved direct children. Root style supplies nested defaults and configured positions stay in their source folder. Exact configured centers survive zoom, viewport clipping, overlaps and drag calculations. Items without positions receive deterministic preview grid centers. The frontend can use each item's position source to distinguish fallback placement from a saved edit.

The preview geometry uses separate folder, canvas and Finder screen coordinates. The project adapter consumes already resolved virtual paths and carries configured background metadata. An actual-image adapter can feed decoded positions into the same geometry without routing them through a project. Asset decoding, text measurement, rendering, and native Finder comparison belong to their respective adapters.

The tests cover the coordinate and design-layout portion of D-GUI-01 and D-V11. The property test checks inverse viewport transforms across the documented test ranges and tolerance. Integration tests use synthetic project metadata, including an intentionally absent copy source and a virtual Applications link.
