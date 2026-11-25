# External copy watch model

`ExternalCopyWatch.tla` checks the external copy watch part of [D-V14](../../../../verification/obligations.json). A watch selects two apps on one managed mount. One app starts with an existing destination and the other starts absent. Each named destination can be absent or have one of two identities and one of two revisions. Up to three destination changes can occur. The final host check can observe a different mount, an already-detached mount, or a stale intent. All and Any policies are checked with and without explicit auto-eject.

| Model action or invariant | Production boundary |
| --- | --- |
| `Observe`, `QuietPeriod` | Baseline identity filters out an existing destination even when its revision changes. An unavailable sample clears the candidate and invalidates its active attempt. A quiet period can mature an observed candidate for verification. [Monitor](../src/watch.rs) |
| `Verify`, `IncompleteVerify` | A full comparison must yield the requested snapshot. Missing or inconclusive results restart the quiet period. The real monitor also matches the receipt to the outstanding attempt token. [Watch transitions](../src/watch.rs) · [Existing bundle verifier](../../../runtime/dmgez-operations/src/install/execute/native/existing.rs) |
| `Recheck`, `FreshCopiesAtRequest` | Only matched snapshots that still pass the final batch check can count. All requires both apps. Any requires at least one. A further check before the host call requires those confirmed snapshots to remain current. [Watch driver](../../../../apps/dmgez-cli/src/mount_command/watch.rs) |
| `Stop`, `ManagedMountOnly`, `BoundedNormalRequest` | Cancellation or deadline expiry before the host call prevents a new request. The driver revalidates the managed session and attachment and permits a single normal detach request. Busy and failed outcomes finish the authorization. An already-detached attachment requires no native request. [Monitor](../src/watch.rs) · [Native detach](../src/system/macos.rs) |

The model assumes an accepted comparison establishes exact bytes and required metadata for the named snapshot. It represents descriptor and tree rechecks with a single revision observation per app. A lost or inconclusive sample may affect either or both apps. Quiet time and deadline expiry are abstract events, and the model makes no fairness claim about which ready app is verified first. It checks the evidence sampled before a native request. A rename or write after those samples, operating system detach semantics, and actual Finder behavior require separate qualification.

## Reproduce

Run from the repository root with Java 21 and the SHA-256-pinned TLA+ Tools v1.8.0 JAR. The checker leaves TLC's deadlock check enabled.

```sh
TLA2TOOLS_JAR=/path/to/tla2tools.jar bash crates/platform/dmgez-mount/verification/check.sh
```

On macOS 14.7.2 arm64 with Temurin 21.0.8, TLC build `2026.11.19.180144` (`bc1b506`) explored 119,328 generated states and 34,716 distinct states from four initial states. The safety search reached depth 14 with an empty queue and no invariant or deadlock failures.

| Configuration | Observed witness |
| --- | --- |
| `AllCopiesWitness.cfg` | With the normal guards intact, both fresh copies pass and All reaches the normal detach request at trace length 12. |
| `OneCopyWitness.cfg` | With the normal guards intact, exactly one fresh copy passes and Any reaches the normal detach request at trace length 9. |
| `IncumbentCopy.cfg` | Allowing the existing destination to count violates `FreshCopiesAtRequest` at trace length 8. Neither destination changed. |
| `StaleCopyRecheck.cfg` | Skipping the last snapshot check violates `FreshCopiesAtRequest` at trace length 10. The confirmed app changes revision on the same inode after recheck. |
| `ReusedMount.cfg` | Ignoring the mount identity violates `ManagedMountOnly` at trace length 10. The requested detach would affect the other mount. |

The first two configurations deliberately negate reachability with the production guards enabled. The last three each disable one guard. The checker requires the expected invariant and a specific action in each trace.
