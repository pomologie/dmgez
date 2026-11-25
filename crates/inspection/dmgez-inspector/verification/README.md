# Remote range read model

`RemoteRead.tla` checks the remote identity part of [D-V15](../../../../verification/obligations.json). One inspection selects one of two endpoints. Each endpoint exposes one of two revisions and can change once during the inspection. Revisions have the same nominal size. The same strong tag can identify different bytes at different endpoints.

| Model action or invariant | Production boundary |
| --- | --- |
| `Issue`, `BoundedReservations`, `Expire` | `State::fetch` charges one request and the byte span plus an EOF probe before sending a GET. The request and body share an absolute deadline. [Range source](../src/remote.rs) · [HTTP adapter](../src/remote/http.rs) |
| `SameOriginPartial`, `ExactRanges` | A single partial response must match the requested interval, exact body length, identity encoding, and strong tag. Invalid range, framing, and status responses do not contribute accepted bytes. [Range source](../src/remote.rs) |
| `StableEntity`, `FailedPrecondition` | Conditional reads send the admitted ETag with `If-Match`. A precondition failure or mismatched entity latches the source as changed. [Range source](../src/remote.rs) |
| `FixedEndpoint`, `FollowedRedirect` | The concrete adapter fixes the explicit endpoint and disables redirects. The mutation models an adapter following a redirect to a different endpoint that returns a valid partial response with a matching tag. [HTTP adapter](../src/remote/http.rs) |
| `ReportRevalidated` | The CLI reads the trailer and index and revalidates with an independent conditional probe before emitting a report. The model includes one optional partition probe. A partition error can be diagnostic, and a source change still stops publication. [Report path](../../../../apps/dmgez-cli/src/inspect.rs) |

The model assumes each origin binds its strong tags to the represented bytes. The read schedule uses five abstract ranges with spans of one or two bytes. Two values each for request, body, and range limits exercise budget exhaustion. Each transport result abstracts one observed response and groups rejected HTTP statuses at the same boundary. Timeout is an abstract event. A subsequent remote change after the final successful probe falls outside that observation. TLS, socket parsing, exact Rust execution, request timing, and MCP approvals require separate evidence.

## Reproduce

Run from the repository root with Java 21 and the SHA-256-pinned TLA+ Tools v1.8.0 JAR. The checker leaves TLC's deadlock check enabled and requires both mutations to violate their intended invariants.

```sh
TLA2TOOLS_JAR=/path/to/tla2tools.jar bash crates/inspection/dmgez-inspector/verification/check.sh
```

On macOS 14.7.2 arm64 with Temurin 21.0.8, TLC build `2026.11.19.180144` (`bc1b506`) explored 13,664 generated states and 5,376 distinct states from 128 initial states. The positive search reached depth 12 with an empty queue and no invariant or deadlock failures.

| Mutation | Observed violation |
| --- | --- |
| `ChangedRevision.cfg` | `StableEntity`, trace length 6. The source admits revision 1, the endpoint changes revision, and the mutation accepts revision 2 for the next range. |
| `RedirectedRead.cfg` | `FixedEndpoint`, trace length 5. The target responds with the same strong tag and total length after the adapter follows a redirect. |
