---- MODULE ExternalCopyWatch ----
EXTENDS Integers, FiniteSets

CONSTANT Mutation
ASSUME Mutation \in {"none", "accept incumbent", "skip last check", "ignore mount"}

Apps == 1..2
None == 0
Absent == <<None, None>>
Unavailable == <<None, 1>>
Snapshots == (1..2) \X (1..2)
Destinations == Snapshots \cup {Absent}
Baselines == [app \in Apps |-> IF app = 1 THEN <<1, 1>> ELSE Absent]
EmptyCandidate == [snapshot |-> Absent, quiet |-> FALSE, matched |-> FALSE]
NoRequest == [
	mount |-> "none",
	intent |-> "none",
	checked |-> Baselines,
	observed |-> Baselines,
	confirmed |-> {}
]
ManagedMount == "managed mount"
OtherMount == "other mount"
Active == {"watching", "verifying", "rechecking", "approved"}
TerminalModes == {"cancelled", "timed out", "failed", "busy", "detached",
	"already detached", "verified"}

VARIABLES selection, autoEject, live, changes, targets, pending, mode, context, request
vars == <<selection, autoEject, live, changes, targets, pending, mode, context, request>>

Init ==
	/\ selection \in {"all", "any"}
	/\ autoEject \in BOOLEAN
	/\ live = Baselines
	/\ changes = 3
	/\ targets = [app \in Apps |-> EmptyCandidate]
	/\ pending = None
	/\ mode = "watching"
	/\ context = "current"
	/\ request = NoRequest

Fresh(snapshot, app) ==
	/\ snapshot \in Snapshots
	/\ snapshot[1] # Baselines[app][1]

Updated(app, observed) ==
	IF observed \notin Snapshots \/ (Mutation # "accept incumbent" /\ ~Fresh(observed, app))
	THEN EmptyCandidate
	ELSE IF observed = targets[app].snapshot
	     THEN targets[app]
	     ELSE [EmptyCandidate EXCEPT !.snapshot = observed]

ObserveBatch(lost) ==
	[app \in Apps |-> IF app \in lost THEN Unavailable ELSE live[app]]

Matched(candidates) == {app \in Apps : candidates[app].matched}

Enough(confirmed) ==
	IF selection = "all" THEN confirmed = Apps ELSE confirmed # {}

QuietUnverified(candidates) ==
	{app \in Apps : candidates[app].quiet /\ ~candidates[app].matched}

Observe ==
	/\ mode \in {"watching", "verifying"}
	/\ \E lost \in SUBSET Apps:
		LET observed == ObserveBatch(lost)
		    nextTargets == [app \in Apps |-> Updated(app, observed[app])]
		    ready == QuietUnverified(nextTargets)
		IN /\ targets' = nextTargets
		   /\ IF mode = "verifying" /\ nextTargets[pending].snapshot = targets[pending].snapshot
		      THEN UNCHANGED <<mode, pending>>
		      ELSE IF Enough(Matched(nextTargets))
		           THEN /\ mode' = "rechecking"
		                /\ pending' = None
		           ELSE IF ready # {}
		                THEN /\ pending' \in ready
		                     /\ mode' = "verifying"
		                ELSE /\ mode' = "watching"
		                     /\ pending' = None
	/\ UNCHANGED <<selection, autoEject, live, changes, context, request>>

QuietPeriod ==
	/\ mode \in {"watching", "verifying"}
	/\ targets' = [app \in Apps |->
		[targets[app] EXCEPT !.quiet = targets[app].snapshot # Absent]]
	/\ UNCHANGED <<selection, autoEject, live, changes, pending, mode, context, request>>

Verify ==
	/\ mode = "verifying"
	/\ targets[pending].snapshot = live[pending]
	/\ targets' = [targets EXCEPT ![pending].matched = TRUE]
	/\ mode' = "watching"
	/\ pending' = None
	/\ UNCHANGED <<selection, autoEject, live, changes, context, request>>

IncompleteVerify ==
	/\ mode = "verifying"
	/\ targets' = [targets EXCEPT ![pending].quiet = FALSE]
	/\ mode' = "watching"
	/\ pending' = None
	/\ UNCHANGED <<selection, autoEject, live, changes, context, request>>

Recheck ==
	/\ mode = "rechecking"
	/\ \E lost \in SUBSET Apps:
		LET retained == Matched(targets)
		    sample == ObserveBatch(lost)
		    observed == [app \in Apps |->
			IF app \in retained /\ sample[app] # targets[app].snapshot
			THEN Unavailable ELSE sample[app]]
		    nextTargets == [app \in Apps |-> Updated(app, observed[app])]
		IN /\ targets' = nextTargets
		   /\ mode' = IF Enough(Matched(nextTargets)) THEN "approved" ELSE "watching"
	/\ UNCHANGED <<selection, autoEject, live, changes, pending, context, request>>

Change(app, revision) ==
	/\ mode \in Active
	/\ changes > 0
	/\ live' = [live EXCEPT ![app] = revision]
	/\ changes' = changes - 1
	/\ UNCHANGED <<selection, autoEject, targets, pending, mode, context, request>>

MoveName ==
	\E app \in Apps, revision \in Destinations:
		/\ live[app][1] # revision[1]
		/\ Change(app, revision)

EditInPlace ==
	\E app \in Apps:
		/\ live[app] \in Snapshots
		/\ live[app][2] = 1
		/\ Change(app, <<live[app][1], 2>>)

ChangeContext ==
	/\ mode = "approved"
	/\ context = "current"
	/\ context' \in {"different mount", "departed mount", "stale intent"}
	/\ UNCHANGED <<selection, autoEject, live, changes, targets, pending, mode, request>>

VerifiedSnapshotsCurrent ==
	\A app \in Matched(targets): live[app] = targets[app].snapshot

NormalCall == [
	mount |-> IF context = "different mount" THEN OtherMount ELSE ManagedMount,
	intent |-> context,
	checked |-> [app \in Apps |-> targets[app].snapshot],
	observed |-> live,
	confirmed |-> Matched(targets)
]

RequestNormal ==
	/\ mode = "approved"
	/\ autoEject
	/\ (context = "current" \/ (Mutation = "ignore mount" /\ context = "different mount"))
	/\ (VerifiedSnapshotsCurrent \/ (Mutation = "skip last check" /\
		\A app \in Matched(targets): live[app][1] = targets[app].snapshot[1]))
	/\ request = NoRequest
	/\ request' = NormalCall
	/\ mode' = "requesting"
	/\ UNCHANGED <<selection, autoEject, live, changes, targets, pending, context>>

ReportVerified ==
	/\ mode = "approved"
	/\ ~autoEject
	/\ context = "current"
	/\ VerifiedSnapshotsCurrent
	/\ mode' = "verified"
	/\ UNCHANGED <<selection, autoEject, live, changes, targets, pending, context, request>>

RefuseRequest ==
	/\ mode = "approved"
	/\ (context # "current" \/ ~VerifiedSnapshotsCurrent)
	/\ mode' = IF autoEject /\ context = "departed mount" THEN "already detached" ELSE "failed"
	/\ UNCHANGED <<selection, autoEject, live, changes, targets, pending, context, request>>

Stop ==
	/\ mode \in Active
	/\ mode' \in {"cancelled", "timed out"}
	/\ pending' = None
	/\ UNCHANGED <<selection, autoEject, live, changes, targets, context, request>>

DetachOutcome ==
	/\ mode = "requesting"
	/\ mode' \in {"detached", "busy", "failed"}
	/\ UNCHANGED <<selection, autoEject, live, changes, targets, pending, context, request>>

Terminal ==
	/\ mode \in TerminalModes
	/\ UNCHANGED vars

Next ==
	\/ Observe
	\/ QuietPeriod
	\/ Verify
	\/ IncompleteVerify
	\/ Recheck
	\/ MoveName
	\/ EditInPlace
	\/ ChangeContext
	\/ RequestNormal
	\/ ReportVerified
	\/ RefuseRequest
	\/ Stop
	\/ DetachOutcome
	\/ Terminal

FreshCopiesAtRequest ==
	request # NoRequest =>
		/\ Enough(request.confirmed)
		/\ \A app \in request.confirmed:
			/\ Fresh(request.observed[app], app)
			/\ request.observed[app] = request.checked[app]

ManagedMountOnly ==
	request # NoRequest =>
		/\ request.mount = ManagedMount
		/\ request.intent = "current"

BoundedNormalRequest ==
	/\ (request # NoRequest => autoEject /\ mode \in {"requesting", "detached", "busy", "failed"})
	/\ (mode \in {"requesting", "detached", "busy"} => request # NoRequest)

NoAllRequest ==
	selection = "all" => request = NoRequest

NoSingleMatchRequest ==
	selection = "any" /\ Cardinality(request.confirmed) = 1 => request = NoRequest

====
