----------------------------- MODULE OrderedBatch -----------------------------
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS MaxJobs, MaxWorkers, MaxCredits, Mutation
VARIABLES input, state

Phase == [admit |-> "admit", fill |-> "fill", read |-> "read", encode |-> "encode",
	consume |-> "consume", write |-> "write", eof |-> "eof", finish |-> "finish",
	ok |-> "ok", error |-> "error"]
Job == [pending |-> "pending", reading |-> "reading", filled |-> "filled",
	encoding |-> "encoding", ready |-> "ready", failed |-> "failed",
	writing |-> "writing", written |-> "written", dropped |-> "dropped"]
Cause == [none |-> "none", budget |-> "budget", pool |-> "pool",
	source |-> "source", codec |-> "codec", sink |-> "sink", cancelled |-> "cancelled"]
Fault == [none |-> "none", completionOrder |-> "completion-order", lostCredit |-> "lost-credit"]

ASSUME /\ MaxJobs \in Nat \ {0}
	/\ MaxWorkers \in Nat \ {0}
	/\ MaxCredits \in Nat
	/\ Mutation \in {Fault.none, Fault.completionOrder, Fault.lostCredit}

Inputs == [jobs : 1..MaxJobs, workers : 1..MaxWorkers, credits : 0..MaxCredits,
	automatic : BOOLEAN]
Jobs == 1..input.jobs
Variables == <<input, state>>
LiveJobs == {Job.reading, Job.filled, Job.encoding, Job.ready, Job.failed, Job.writing}
Results == {Job.ready, Job.failed}
Terminal == state.phase \in {Phase.ok, Phase.error}
ActiveWidth ==
	IF input.automatic /\ input.credits < input.workers
	THEN IF input.credits = 0 THEN 1 ELSE input.credits
	ELSE input.workers

Init == /\ input \in Inputs
	/\ state = [phase |-> Phase.admit, width |-> 0, cancelled |-> FALSE,
		cause |-> Cause.none, lastAdmitted |-> 0,
		jobs |-> [j \in Jobs |-> Job.pending], held |-> {},
		released |-> [j \in Jobs |-> 0], completions |-> <<>>, emitted |-> <<>>]

(* The operation retains each reservation until its result is consumed or dropped. *)
FirstHeld == CHOOSE j \in state.held : \A k \in state.held : j <= k
DroppedReservations ==
	IF Mutation = Fault.lostCredit /\ state.held /= {}
	THEN state.held \ {FirstHeld}
	ELSE state.held

Abort(cause) == state' = [state EXCEPT !.phase = Phase.error, !.cause = cause,
	!.jobs = [j \in Jobs |-> IF j \in state.held THEN Job.dropped ELSE state.jobs[j]],
	!.held = state.held \ DroppedReservations,
	!.released = [j \in Jobs |-> state.released[j] + (IF j \in DroppedReservations THEN 1 ELSE 0)]]

RequestCancel == /\ ~Terminal
	/\ ~state.cancelled
	/\ state' = [state EXCEPT !.cancelled = TRUE]

Admit == /\ state.phase = Phase.admit
	/\ IF state.cancelled THEN Abort(Cause.cancelled)
		ELSE IF ActiveWidth > input.credits THEN Abort(Cause.budget)
		ELSE \/ state' = [state EXCEPT !.phase = Phase.fill, !.width = ActiveWidth]
			\/ /\ ActiveWidth > 1
				/\ Abort(Cause.pool)

Fill == /\ state.phase = Phase.fill
	/\ IF state.lastAdmitted < input.jobs /\ Cardinality(state.held) < state.width
		THEN IF state.cancelled THEN Abort(Cause.cancelled)
			ELSE LET j == state.lastAdmitted + 1 IN
				state' = [state EXCEPT !.phase = Phase.read, !.lastAdmitted = j,
					!.jobs[j] = Job.reading, !.held = state.held \cup {j}]
		ELSE /\ state.held /= {}
			/\ state' = [state EXCEPT !.phase = Phase.encode]

Read == /\ state.phase = Phase.read
	/\ LET j == state.lastAdmitted IN
		\/ state' = [state EXCEPT !.phase = Phase.fill, !.jobs[j] = Job.filled]
		\/ Abort(Cause.source)

RecordResult(j, result) == state' = [state EXCEPT !.jobs[j] = result,
	!.completions = Append(state.completions, j)]

StartEncode(j) == /\ state.jobs[j] = Job.filled
	/\ IF state.cancelled THEN RecordResult(j, Job.failed)
		ELSE state' = [state EXCEPT !.jobs[j] = Job.encoding]

CompleteEncode(j) == /\ state.jobs[j] = Job.encoding
	/\ \E result \in Results : RecordResult(j, result)

Encode == /\ state.phase = Phase.encode
	/\ \/ (\E j \in state.held : StartEncode(j) \/ CompleteEncode(j))
		\/ /\ state.held /= {}
			/\ \A j \in state.held : state.jobs[j] \in Results
			/\ state' = [state EXCEPT !.phase = Phase.consume]

(* Indexed collect exposes input order even when the workers finish in another order. *)
NextResult ==
	IF Mutation = Fault.completionOrder
	THEN Head(SelectSeq(state.completions, LAMBDA j : j \in state.held))
	ELSE Len(state.emitted) + 1

Consume == /\ state.phase = Phase.consume
	/\ IF state.held = {}
		THEN state' = [state EXCEPT !.completions = <<>>,
			!.phase = IF state.lastAdmitted = input.jobs THEN Phase.eof ELSE Phase.fill]
		ELSE LET j == NextResult IN
			/\ j \in state.held
			/\ IF state.cancelled THEN Abort(Cause.cancelled)
				ELSE IF state.jobs[j] = Job.failed THEN Abort(Cause.codec)
				ELSE /\ state.jobs[j] = Job.ready
					/\ state' = [state EXCEPT !.phase = Phase.write, !.jobs[j] = Job.writing]

Write == /\ state.phase = Phase.write
	/\ \E j \in state.held :
		/\ state.jobs[j] = Job.writing
		/\ \/ state' = [state EXCEPT !.phase = Phase.consume, !.jobs[j] = Job.written,
				!.emitted = Append(state.emitted, j), !.held = state.held \ {j},
				!.released[j] = state.released[j] + 1]
			\/ Abort(Cause.sink)

CheckEof == /\ state.phase = Phase.eof
	/\ \/ state' = [state EXCEPT !.phase = Phase.finish]
		\/ Abort(Cause.source)

Finish == /\ state.phase = Phase.finish
	/\ IF state.cancelled THEN Abort(Cause.cancelled)
		ELSE \/ state' = [state EXCEPT !.phase = Phase.ok]
			\/ Abort(Cause.sink)

Progress == /\ UNCHANGED input
	/\ (Admit \/ Fill \/ Read \/ Encode \/ Consume \/ Write \/ CheckEof \/ Finish)
Next == \/ Progress
	\/ /\ UNCHANGED input
		/\ RequestCancel
	\/ /\ Terminal
		/\ UNCHANGED Variables

Spec == Init /\ [][Next]_Variables /\ WF_Variables(Progress)

TypeOk == /\ input \in Inputs
	/\ state \in [phase : {Phase[k] : k \in DOMAIN Phase}, width : 0..MaxWorkers,
		cancelled : BOOLEAN, cause : {Cause[k] : k \in DOMAIN Cause},
		lastAdmitted : 0..input.jobs, jobs : [Jobs -> {Job[k] : k \in DOMAIN Job}],
		held : SUBSET Jobs, released : [Jobs -> 0..1],
		completions : Seq(Jobs), emitted : Seq(Jobs)]
	/\ Len(state.completions) <= input.jobs
	/\ Len(state.emitted) <= input.jobs

CreditsBounded == /\ Cardinality(state.held) <= input.credits
	/\ Cardinality(state.held) <= state.width

CreditLifecycle == \A j \in Jobs :
	/\ (j \in state.held) = (state.jobs[j] \in LiveJobs)
	/\ IF j <= state.lastAdmitted
		THEN (IF j \in state.held THEN 1 ELSE 0) + state.released[j] = 1
		ELSE /\ state.jobs[j] = Job.pending
			/\ state.released[j] = 0

LiveBatchIsContiguous == ~Terminal =>
	state.held = ((Len(state.emitted) + 1)..state.lastAdmitted)

CollectedBeforeWriting == state.phase \in {Phase.consume, Phase.write} =>
	\A j \in state.held : state.jobs[j] \in (Results \cup {Job.writing})

OrderedConsumption == state.emitted = [j \in 1..Len(state.emitted) |-> j]

CreditsReleasedOnTermination == Terminal =>
	/\ state.held = {}
	/\ \A j \in 1..state.lastAdmitted : state.released[j] = 1

NoPartialSuccess == /\ (state.phase = Phase.error) = (state.cause /= Cause.none)
	/\ state.phase = Phase.ok =>
		/\ ~state.cancelled
		/\ state.lastAdmitted = input.jobs
		/\ state.emitted = [j \in Jobs |-> j]
		/\ \A j \in Jobs : state.jobs[j] = Job.written

EventuallyTerminal == <>Terminal
=============================================================================
