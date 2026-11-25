---------------------------- MODULE Publication ----------------------------
EXTENDS FiniteSets, Naturals

CONSTANTS Writers, Absent, Incumbent, Mutation

Production == "production"
PublishFailedWrite == "publish failed write"
ReplaceWinner == "replace winner"

ASSUME /\ Cardinality(Writers) = 2
       /\ Absent # Incumbent
       /\ {Absent, Incumbent} \intersect Writers = {}
       /\ Mutation \in {Production, PublishFailedWrite, ReplaceWinner}

BeforeAdmission == "before admission"
Create == "create"
Write == "write"
Sync == "sync"
Rewind == "rewind"
Check == "check"
Ready == "ready"
WriteFailed == "write failed"
Unwinding == "unwinding"
Persisted == "persisted"
Aborted == "aborted"
Crashed == "crashed"
Returned == "returned"

Phases == {BeforeAdmission, Create, Write, Sync, Rewind, Check, Ready,
           WriteFailed, Unwinding, Persisted, Aborted, Crashed, Returned}
Terminal == {Aborted, Crashed, Returned}

Missing == "missing"
Empty == "empty"
Prefix == "prefix"
Complete == "complete"
Payloads == {Missing, Empty, Prefix, Complete}

NoCrash == "no crash"
ProcessCrashed == "process crashed"
AwaitingRestart == "power lost"
Restarted == "restarted"
CrashStates == {NoCrash, ProcessCrashed, AwaitingRestart, Restarted}

VARIABLES attempts, initialOutput, output, commits, cancelTo, crash, orphans

vars == <<attempts, initialOutput, output, commits, cancelTo, crash, orphans>>
shared == <<initialOutput, output, commits, cancelTo, crash, orphans>>

Attempt(phase, data, syncedData) ==
    [phase |-> phase, data |-> data, syncedData |-> syncedData]

Output(owner, data, synced, checked) ==
    [owner |-> owner, data |-> data, synced |-> synced, checked |-> checked]

NoOutput == Output(Absent, Missing, FALSE, FALSE)
OldOutput == Output(Incumbent, Complete, TRUE, TRUE)

Init ==
    /\ attempts = [w \in Writers |-> Attempt(BeforeAdmission, Missing, Missing)]
    /\ initialOutput \in {NoOutput, OldOutput}
    /\ output = initialOutput
    /\ commits = {}
    /\ cancelTo = Absent
    /\ crash = NoCrash
    /\ orphans = {}

InitAbsent == Init /\ initialOutput = NoOutput

SetAttempt(w, state) == attempts' = [attempts EXCEPT ![w] = state]
Advance(w, phase) == attempts' = [attempts EXCEPT ![w].phase = phase]
At(w, phase) == attempts[w].phase = phase
Live(w) == attempts[w].phase \notin Terminal
HasStage(w) == attempts[w].data # Missing

Admit(w) ==
    /\ At(w, BeforeAdmission)
    /\ cancelTo # w
    /\ Advance(w, Create)
    /\ UNCHANGED shared

CreateStage(w) ==
    /\ At(w, Create)
    /\ SetAttempt(w, Attempt(Write, Empty, Missing))
    /\ UNCHANGED shared

WritePrefix(w) ==
    /\ At(w, Write)
    /\ attempts[w].data = Empty
    /\ cancelTo # w
    /\ SetAttempt(w, Attempt(Write, Prefix, Missing))
    /\ UNCHANGED shared

FinishWrite(w) ==
    /\ At(w, Write)
    /\ cancelTo # w
    /\ SetAttempt(w, Attempt(Sync, Complete, Missing))
    /\ UNCHANGED shared

SyncStage(w) ==
    /\ At(w, Sync)
    /\ SetAttempt(w, Attempt(Rewind, attempts[w].data, attempts[w].data))
    /\ UNCHANGED shared

RewindStage(w) ==
    /\ At(w, Rewind)
    /\ Advance(w, Check)
    /\ UNCHANGED shared

CheckStage(w) ==
    /\ At(w, Check)
    /\ cancelTo # w
    /\ Advance(w, Ready)
    /\ UNCHANGED shared

RequestCancel(w) ==
    /\ cancelTo = Absent
    /\ Live(w)
    /\ cancelTo' = w
    /\ UNCHANGED <<attempts, initialOutput, output, commits, crash, orphans>>

ObserveCancel(w) ==
    /\ cancelTo = w
    /\ attempts[w].phase \in {BeforeAdmission, Write, Check}
    /\ Advance(w, Unwinding)
    /\ UNCHANGED shared

(* The write callback returns ENOSPC after emitting a prefix. *)
StorageFull(w) ==
    /\ At(w, Write)
    /\ attempts[w].data = Prefix
    /\ Advance(w, WriteFailed)
    /\ UNCHANGED shared

(* Any other stage can return an error. The Ready case includes
   AlreadyExists when a concurrent writer reaches the destination first. *)
StageError(w) ==
    /\ attempts[w].phase \in {BeforeAdmission, Create, Write, Sync, Rewind, Check, Ready}
    /\ Advance(w, Unwinding)
    /\ UNCHANGED shared

AbandonStage(w) ==
    /\ attempts[w].phase \in {WriteFailed, Unwinding}
    /\ SetAttempt(w, Attempt(Aborted, Missing, Missing))
    /\ orphans' \in IF HasStage(w) THEN {orphans, orphans \union {w}} ELSE {orphans}
    /\ UNCHANGED <<initialOutput, output, commits, cancelTo, crash>>

(* No cancellation polling occurs after the successful final check.
   Sync applies to data. The destination entry becomes visible here. *)
CommitStage(w) ==
    /\ \/ At(w, Ready)
       \/ Mutation = PublishFailedWrite /\ At(w, WriteFailed)
    /\ \/ output = NoOutput
       \/ Mutation = ReplaceWinner /\ output.owner \in Writers
    /\ output' = Output(w, attempts[w].data, attempts[w].syncedData = Complete, At(w, Ready))
    /\ commits' = commits \union {w}
    /\ SetAttempt(w, Attempt(Persisted, Missing, Missing))
    /\ orphans' \in {orphans, orphans \union {w}}
    /\ UNCHANGED <<initialOutput, cancelTo, crash>>

ReturnSuccess(w) ==
    /\ At(w, Persisted)
    /\ Advance(w, Returned)
    /\ UNCHANGED shared

CrashProcess(w) ==
    /\ crash = NoCrash
    /\ Live(w)
    /\ crash' = ProcessCrashed
    /\ SetAttempt(w, Attempt(Crashed, Missing, Missing))
    /\ orphans' = IF HasStage(w) THEN orphans \union {w} ELSE orphans
    /\ UNCHANGED <<initialOutput, output, commits, cancelTo>>

LosePower ==
    /\ crash = NoCrash
    /\ crash' = AwaitingRestart
    /\ attempts' = [w \in Writers |-> IF Live(w) THEN Attempt(Crashed, Missing, Missing)
                                      ELSE attempts[w]]
    /\ orphans' \in SUBSET (orphans \union {w \in Writers : HasStage(w)})
    (* No directory sync requires considering loss of the new directory entry.
       An already durable incumbent remains in both recovery choices. *)
    /\ output' \in {initialOutput, output}
    /\ UNCHANGED <<initialOutput, commits, cancelTo>>

Restart ==
    /\ crash = AwaitingRestart
    /\ crash' = Restarted
    /\ UNCHANGED <<attempts, initialOutput, output, commits, cancelTo, orphans>>

WriterStep(w) ==
    \/ Admit(w)
    \/ CreateStage(w)
    \/ WritePrefix(w)
    \/ FinishWrite(w)
    \/ SyncStage(w)
    \/ RewindStage(w)
    \/ CheckStage(w)
    \/ RequestCancel(w)
    \/ ObserveCancel(w)
    \/ StorageFull(w)
    \/ StageError(w)
    \/ AbandonStage(w)
    \/ CommitStage(w)
    \/ ReturnSuccess(w)
    \/ CrashProcess(w)

(* Quiescence is legal after both attempts end and a power loss has recovered. *)
Quiescent ==
    /\ crash # AwaitingRestart
    /\ \A w \in Writers : attempts[w].phase \in Terminal
    /\ UNCHANGED vars

Next ==
    \/ /\ crash # AwaitingRestart
       /\ \E w \in Writers : WriterStep(w)
    \/ LosePower
    \/ Restart
    \/ Quiescent

TypeOk ==
    /\ attempts \in [Writers -> [phase : Phases, data : Payloads, syncedData : {Missing, Complete}]]
    /\ initialOutput \in {NoOutput, OldOutput}
    /\ output \in [owner : Writers \union {Absent, Incumbent}, data : Payloads,
                  synced : BOOLEAN, checked : BOOLEAN]
    /\ commits \subseteq Writers
    /\ cancelTo \in Writers \union {Absent}
    /\ crash \in CrashStates
    /\ orphans \subseteq Writers

CompleteCheckedPublication ==
    output.owner \in Writers =>
        /\ output.owner \in commits
        /\ output.data = Complete
        /\ output.synced
        /\ output.checked

NoClobber ==
    /\ (initialOutput = OldOutput => output = OldOutput)
    /\ Cardinality(commits) <= 1

=============================================================================
