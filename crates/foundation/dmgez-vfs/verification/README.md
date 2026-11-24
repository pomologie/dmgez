# Partition window verification

Run `bash verification/check.sh` from `crates/foundation/dmgez-vfs` with the repository's Kani toolchain installed. The harness calls the production window translator over up to four nested windows and compares every admission and endpoint with widened `u128` arithmetic. The parent range is assumed well ordered. The proof covers logical byte arithmetic for D-V09. Native and adversarial tests check map admission and actual source reads separately.
