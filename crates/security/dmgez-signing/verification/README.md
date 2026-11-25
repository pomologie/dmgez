# Staged release authorization model

`StagedRelease.tla` checks the TLC portion of [D-V12](../../../../verification/obligations.json) for one authorized release and one durable notarization record. The model covers two source revisions, two possible submission IDs, and unsigned, signed, and stapled envelopes. Fourteen valid release plans vary source and assembled-app verification, disk signing, notarization, stapling, and native assessment. Each plan starts in both revisions. A run can observe one source revision change and one staged file replacement. The mutation profiles can consume a stale ticket or make a second upload call.

## Production mapping

| Action | Production boundary |
| --- | --- |
| `Prepare`, `BuildImage` | Source app verification checks the captured manifest before the unsigned writer stages the image. The manifest and stage checks bind the sealed revision. [Release driver](../../../../apps/dmgez-cli/src/project_build/release.rs) |
| `VerifyUnsigned`, `VerifyApps` | Required unsigned and reconstructed app checks admit evidence for the exact staged digest. The model records a separate proof for each selected check. [Release receipts](../src/release_plan.rs) |
| `Sign` | Signing requires the unsigned check and, when selected, both app gates. The app and unsigned evidence stays bound to the volume revision while the native signer appends an envelope. [Release plan](../src/release_plan/plan.rs) · [Native image checks](../src/native/disk/image.rs) |
| `ReserveSubmission`, `Submit` | Notarization persists a fresh digest-bound reservation before the upload. An ambiguous response can leave a reservation without an ID. [Submission flow](../src/notarization/flow.rs) |
| `UnknownPoll`, `PendingPoll`, `ReceiveTerminal` | Pending resumes use a bound ID. Recovery from an ambiguous reservation takes an explicit candidate ID and requires a matching terminal signed-artifact digest. Once verified, a terminal record is reusable locally without another remote lookup or rewrite. [Status and receipt flow](../src/notarization/flow.rs) |
| `Staple`, `VerifyFinal`, `AssessFinal`, `PublishImage` | Stapling uses the accepted submission for the observed signed input. Publication requires the exact final image, source revalidation, and all selected checks. [Release driver](../../../../apps/dmgez-cli/src/project_build/release.rs) |
| `Fail`, `Cancel` | The active release stops admitting further actions. The reservation and known submission evidence survive local cancellation. [Release run](../src/release_plan.rs) |

Digest tokens abstract exact bytes and length and assume no collisions. The selected backend is fixed. Each successful adapter action assumes its byte checks, source observations, or external evidence are correct at the admission boundary. The model checks sampled replacement and cancellation between such actions. A rejected receipt has no successful transition, and an unsuccessful poll may return an unknown result. The model allows terminal stuttering and makes no liveness claim about Apple's service. Cryptography, protected Apple-service qualification, process crashes within a filesystem operation, and hostile changes within one adapter call require separate evidence.

## Reproduction and evidence

Use Java 21 and the TLA+ Tools v1.8.0 JAR pinned by SHA-256 in the checker. Run from the repository root.

```sh
TLA2TOOLS_JAR=/path/to/tla2tools.jar bash crates/security/dmgez-signing/verification/check.sh
```

On macOS 14.7.2 arm64 with Temurin 21.0.8, TLC build `2026.11.19.180144` (`bc1b506`) explored 20,574 generated states and 11,088 distinct states from 28 initial states. The positive search reached depth 15 with an empty queue and no invariant or deadlock failures.

| Mutation configuration | Expected and observed violation |
| --- | --- |
| `StaleTicket.cfg` | `StaplingAuthorized`, trace length 9. The accepted log binds signed revision 1. The stage is replaced with signed revision 2 and the mutation reuses revision 1's ticket. |
| `BlindRetry.cfg` | `SingleSubmission`, trace length 7. The first upload has an unknown outcome and no bound ID. The mutation submits again using the same reservation. |

The checker requires the TLC invariant-violation exit code and the named invariant from each mutation. These results exercise the bounded model. The production [release machine tests](../src/release_plan/tests.rs) and [notarization workflow tests](../src/notarization/flow/tests.rs) provide separate executable evidence.
