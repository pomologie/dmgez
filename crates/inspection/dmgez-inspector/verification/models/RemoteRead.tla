---- MODULE RemoteRead ----
EXTENDS Integers, Sequences

CONSTANT Mutation
ASSUME Mutation \in {"none", "ignore revision", "follow redirect"}

Endpoints == {"first", "second"}
Revisions == 1..2
Modes == {"ready", "pending", "rejected", "expired", "published"}
ByteInterval(first, end) == [first |-> first, end |-> end]
Ranges == [
	admit |-> ByteInterval(0, 1),
	trailer |-> ByteInterval(5, 7),
	index |-> ByteInterval(3, 5),
	disk |-> ByteInterval(1, 3),
	final |-> ByteInterval(0, 1)
]
Size(interval) == interval.end - interval.first
EntityBytes == 7
EOFProbeBytes == 1
ReadMetadata == <<"admit", "trailer", "index">>

VARIABLES endpoint, versions, operations, limits, remaining, accepted, step, mode, changed
vars == <<endpoint, versions, operations, limits, remaining, accepted, step, mode, changed>>

Init ==
	/\ endpoint \in Endpoints
	/\ versions \in [Endpoints -> Revisions]
	/\ operations \in {ReadMetadata \o <<"final">>, ReadMetadata \o <<"disk", "final">>}
	/\ limits \in [requests : {4, 5}, body : {10, 13}, range : {1, 2}]
	/\ remaining = limits
	/\ accepted = <<>>
	/\ step = 1
	/\ mode = "ready"
	/\ changed = FALSE

Operation == operations[step]
Requested == Ranges[Operation]
Reserved == Size(Requested) + EOFProbeBytes
IsAdmission == Operation = "admit"
HasCredit ==
	/\ Requested.end <= EntityBytes
	/\ Size(Requested) <= limits.range
	/\ remaining.requests > 0
	/\ remaining.body >= Reserved

Issue ==
	/\ mode = "ready"
	/\ ~changed
	/\ HasCredit
	/\ mode' = "pending"
	/\ remaining' = [remaining EXCEPT !.requests = @ - 1, !.body = @ - Reserved]
	/\ UNCHANGED <<endpoint, versions, operations, limits, accepted, step, changed>>

Refuse(didChange) ==
	/\ changed' = (changed \/ didChange)
	/\ IF Operation = "disk"
	   THEN /\ mode' = "ready"
	        /\ step' = step + 1
	   ELSE /\ mode' = "rejected"
	        /\ UNCHANGED step
	/\ UNCHANGED <<endpoint, versions, operations, limits, remaining, accepted>>

RefuseBeforeIssue ==
	/\ mode = "ready"
	/\ (changed \/ ~HasCredit)
	/\ Refuse(FALSE)

RefuseStatus ==
	/\ mode = "pending"
	/\ Refuse(FALSE)

FailedPrecondition ==
	/\ mode = "pending"
	/\ ~IsAdmission
	/\ versions[endpoint] # accepted[1].revision
	/\ Refuse(TRUE)

Response(origin, case) == [
	origin |-> origin,
	revision |-> versions[origin],
	total |-> IF case = "wrong total" THEN EntityBytes + 1 ELSE EntityBytes,
	interval |-> IF case = "shifted" THEN ByteInterval(Requested.first + 1, Requested.end + 1)
		ELSE Requested,
	count |-> IF case = "short body" THEN Size(Requested) - 1 ELSE Size(Requested),
	strong |-> case # "weak tag",
	identityEncoding |-> case # "encoded",
	conditional |-> IF IsAdmission THEN 0 ELSE accepted[1].revision,
	operation |-> Operation
]

HasIdentity(reply) ==
	IF IsAdmission THEN TRUE
	ELSE /\ (Mutation = "ignore revision" \/ reply.revision = accepted[1].revision)
	     /\ reply.total = accepted[1].total

Keep(reply) ==
	/\ accepted' = Append(accepted, reply)
	/\ IF Operation = "final"
	   THEN /\ mode' = "published"
	        /\ UNCHANGED step
	   ELSE /\ mode' = "ready"
	        /\ step' = step + 1
	/\ UNCHANGED <<endpoint, versions, operations, limits, remaining, changed>>

AdmitPartial(reply) ==
	IF \/ ~reply.strong
	   \/ ~reply.identityEncoding
	   \/ reply.interval.end > reply.total
	THEN Refuse(FALSE)
	ELSE IF ~HasIdentity(reply)
		THEN Refuse(TRUE)
		ELSE IF reply.interval # Requested \/ reply.count # Size(Requested)
			THEN Refuse(FALSE)
			ELSE Keep(reply)

SameOriginPartial ==
	/\ mode = "pending"
	/\ \E case \in {"complete", "shifted", "short body", "weak tag", "encoded", "wrong total"}:
		AdmitPartial(Response(endpoint, case))

FollowedRedirect ==
	/\ mode = "pending"
	/\ ~IsAdmission
	/\ Mutation = "follow redirect"
	/\ \E origin \in Endpoints \ {endpoint}: AdmitPartial(Response(origin, "complete"))

ReplaceRevision ==
	/\ mode \in {"ready", "pending"}
	/\ \E origin \in Endpoints:
		/\ versions[origin] = 1
		/\ versions' = [versions EXCEPT ![origin] = 2]
	/\ UNCHANGED <<endpoint, operations, limits, remaining, accepted, step, mode, changed>>

Expire ==
	/\ mode \in {"ready", "pending"}
	/\ mode' = "expired"
	/\ UNCHANGED <<endpoint, versions, operations, limits, remaining, accepted, step, changed>>

Terminal ==
	/\ mode \in {"rejected", "expired", "published"}
	/\ UNCHANGED vars

Next ==
	\/ Issue
	\/ RefuseBeforeIssue
	\/ RefuseStatus
	\/ FailedPrecondition
	\/ SameOriginPartial
	\/ FollowedRedirect
	\/ ReplaceRevision
	\/ Expire
	\/ Terminal

FixedEndpoint ==
	\A index \in DOMAIN accepted: accepted[index].origin = endpoint

StableEntity ==
	\A index \in 2..Len(accepted):
		/\ accepted[index].revision = accepted[1].revision
		/\ accepted[index].total = accepted[1].total
		/\ accepted[index].conditional = accepted[1].revision

ExactRanges ==
	\A index \in DOMAIN accepted:
		LET reply == accepted[index] IN
			/\ reply.strong
			/\ reply.identityEncoding
			/\ reply.interval = Ranges[reply.operation]
			/\ reply.count = Size(reply.interval)
			/\ reply.interval.end <= reply.total

BoundedReservations ==
	/\ remaining.requests \in 0..limits.requests
	/\ remaining.body \in 0..limits.body
	/\ Len(accepted) <= limits.requests - remaining.requests

ReportRevalidated ==
	mode = "published" =>
		/\ ~changed
		/\ Len(accepted) >= Len(ReadMetadata) + 1
		/\ \A index \in DOMAIN ReadMetadata: accepted[index].operation = ReadMetadata[index]
		/\ accepted[Len(accepted)].operation = "final"

====
