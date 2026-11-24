--------------------------- MODULE CheckedReplace ---------------------------
EXTENDS FiniteSets, Naturals, Sequences

CONSTANTS Writers, Absent, Predecessor, SourceAlias, NonRegular, ChangedTarget,
          OutputDirectory, SourceDirectory, UnavailableDirectory, Environment

Cooperative == "cooperative"
ObservedChanges == "observed changes"
UntrustedLateChange == "untrusted late change"

ASSUME /\ Cardinality(Writers) = 2
       /\ Cardinality(Writers \union {Absent, Predecessor, SourceAlias, NonRegular,
                                       ChangedTarget}) = 7
       /\ Cardinality({OutputDirectory, SourceDirectory, UnavailableDirectory}) = 3
       /\ Environment \in {Cooperative, ObservedChanges, UntrustedLateChange}

BeforeLock == "before lock"
Capture == "capture"
Authorize == "authorize"
Create == "create"
Write == "write"
Sync == "sync"
Rewind == "rewind"
Validate == "validate"
RecheckStage == "recheck stage"
RecheckDirectory == "recheck directory"
RecheckTarget == "recheck target"
Ready == "ready"
Persisted == "persisted"
Succeeded == "succeeded"
Aborted == "aborted"
Busy == "busy"
Crashed == "crashed"

LockedPhases == {Capture, Authorize, Create, Write, Sync, Rewind, Validate,
                 RecheckStage, RecheckDirectory, RecheckTarget, Ready, Persisted}
Terminal == {Succeeded, Aborted, Busy, Crashed}
Phases == {BeforeLock} \union LockedPhases \union Terminal

NoStage == "no stage"
OwnedStage == "owned stage"
SubstitutedStage == "substituted stage"
StageEntries == {NoStage, OwnedStage, SubstitutedStage}

RegularTargets == Writers \union {Predecessor, SourceAlias, ChangedTarget}
Targets == RegularTargets \union {Absent, NonRegular}
Directories == {OutputDirectory, SourceDirectory, UnavailableDirectory}

NoEdit == "no edit"
StageEdit == "stage edit"
DirectoryEdit == "directory edit"
TargetEdit == "target edit"
LateCreate == "late create"
LateOverwrite == "late overwrite"
EditKinds == {NoEdit, StageEdit, DirectoryEdit, TargetEdit, LateCreate, LateOverwrite}

Initial == "initial"
FilesystemEdit == "external target edit"
Install == "install"

VARIABLES attempts, lock, directory, target, history, edit, cancelled, crashed

vars == <<attempts, lock, directory, target, history, edit, cancelled, crashed>>
shared == <<lock, directory, target, history, edit, cancelled, crashed>>

IdleAttempt ==
    [phase |-> BeforeLock, expectedDirectory |-> OutputDirectory, expectedTarget |-> Absent,
     authorized |-> FALSE, stage |-> NoStage, complete |-> FALSE, synced |-> FALSE,
     validated |-> FALSE]

Change(kind, writer, before, after, safe) ==
    [kind |-> kind, writer |-> writer, before |-> before, after |-> after, safe |-> safe]

Init ==
    /\ attempts = [w \in Writers |-> IdleAttempt]
    /\ lock = Absent
    /\ directory \in {OutputDirectory, SourceDirectory}
    /\ target \in {Absent, Predecessor, SourceAlias, NonRegular}
    /\ history = <<Change(Initial, Absent, target, target, TRUE)>>
    /\ edit = [kind |-> NoEdit, writer |-> Absent]
    /\ cancelled = Absent
    /\ crashed = Absent

InitUntrusted ==
    /\ Init
    /\ directory = OutputDirectory
    /\ target = Predecessor

At(w, phase) == attempts[w].phase = phase
Advance(w, phase) == attempts' = [attempts EXCEPT ![w].phase = phase]

AcquireDirectory(w) ==
    /\ At(w, BeforeLock)
    /\ directory # UnavailableDirectory
    /\ lock = Absent
    /\ cancelled # w
    /\ lock' = w
    /\ attempts' = [attempts EXCEPT ![w].phase = Capture,
                                    ![w].expectedDirectory = directory]
    /\ UNCHANGED <<directory, target, history, edit, cancelled, crashed>>

RejectBusy(w) ==
    /\ At(w, BeforeLock)
    /\ lock \in Writers \ {w}
    /\ Advance(w, Busy)
    /\ UNCHANGED shared

CaptureTarget(w) ==
    /\ At(w, Capture)
    /\ target \in RegularTargets \union {Absent}
    /\ attempts' = [attempts EXCEPT ![w].phase = Authorize, ![w].expectedTarget = target]
    /\ UNCHANGED shared

AuthorizeTarget(w) ==
    /\ At(w, Authorize)
    /\ cancelled # w
    /\ attempts[w].expectedDirectory # SourceDirectory
    /\ attempts[w].expectedTarget # SourceAlias
    /\ attempts' = [attempts EXCEPT ![w].phase = Create, ![w].authorized = TRUE]
    /\ UNCHANGED shared

CreateStage(w) ==
    /\ At(w, Create)
    /\ attempts' = [attempts EXCEPT ![w].phase = Write, ![w].stage = OwnedStage]
    /\ UNCHANGED shared

FinishWrite(w) ==
    /\ At(w, Write)
    /\ cancelled # w
    /\ attempts' = [attempts EXCEPT ![w].phase = Sync, ![w].complete = TRUE]
    /\ UNCHANGED shared

SyncStage(w) ==
    /\ At(w, Sync)
    /\ attempts' = [attempts EXCEPT ![w].phase = Rewind, ![w].synced = TRUE]
    /\ UNCHANGED shared

RewindStage(w) ==
    /\ At(w, Rewind)
    /\ Advance(w, Validate)
    /\ UNCHANGED shared

ValidateStage(w) ==
    /\ At(w, Validate)
    /\ cancelled # w
    /\ attempts' = [attempts EXCEPT ![w].phase = RecheckStage, ![w].validated = TRUE]
    /\ UNCHANGED shared

CheckStageIdentity(w) ==
    /\ At(w, RecheckStage)
    /\ attempts[w].stage = OwnedStage
    /\ Advance(w, RecheckDirectory)
    /\ UNCHANGED shared

CheckDirectoryIdentity(w) ==
    /\ At(w, RecheckDirectory)
    /\ directory = attempts[w].expectedDirectory
    /\ Advance(w, RecheckTarget)
    /\ UNCHANGED shared

CheckTargetStamp(w) ==
    /\ At(w, RecheckTarget)
    /\ target = attempts[w].expectedTarget
    /\ Advance(w, Ready)
    /\ UNCHANGED shared

RequestCancel(w) ==
    /\ cancelled = Absent
    /\ attempts[w].phase \notin Terminal
    /\ cancelled' = w
    /\ UNCHANGED <<attempts, lock, directory, target, history, edit, crashed>>

(* Represents a fault, a failed guard, or cleanup after cancellation.
   Drop can leave an owned stage if unlink fails. A substituted name survives. *)
Abort(w) ==
    /\ attempts[w].phase \in ({BeforeLock} \union LockedPhases) \ {Persisted}
    /\ \E remaining \in IF attempts[w].stage = OwnedStage
                         THEN {NoStage, OwnedStage} ELSE {attempts[w].stage} :
        attempts' = [attempts EXCEPT ![w].phase = Aborted, ![w].stage = remaining]
    /\ lock' = IF lock = w THEN Absent ELSE lock
    /\ UNCHANGED <<directory, target, history, edit, cancelled, crashed>>

(* Record the actual syscall inputs independently of the earlier gate.
   A present target uses ordinary rename, so this predicate is an invariant
   of the trusted environment, not a new guard imposed on the syscall. *)
SafeInstall(w) ==
    /\ lock = w
    /\ directory = attempts[w].expectedDirectory
    /\ target = attempts[w].expectedTarget
    /\ attempts[w].authorized
    /\ attempts[w].stage = OwnedStage
    /\ attempts[w].complete
    /\ attempts[w].synced
    /\ attempts[w].validated

Commit(w) ==
    /\ At(w, Ready)
    /\ IF attempts[w].expectedTarget = Absent THEN target = Absent ELSE TRUE
    /\ target' = w
    /\ history' = Append(history, Change(Install, w, target, target', SafeInstall(w)))
    /\ attempts' = [attempts EXCEPT ![w].phase = Persisted, ![w].stage = NoStage]
    /\ UNCHANGED <<lock, directory, edit, cancelled, crashed>>

ReturnSuccess(w) ==
    /\ At(w, Persisted)
    /\ Advance(w, Succeeded)
    /\ lock' = Absent
    /\ UNCHANGED <<directory, target, history, edit, cancelled, crashed>>

CrashProcess(w) ==
    /\ crashed = Absent
    /\ lock = w
    /\ Advance(w, Crashed)
    /\ crashed' = w
    /\ lock' = Absent
    /\ UNCHANGED <<directory, target, history, edit, cancelled>>

BeforeStageRecheck == {Validate, RecheckStage}
BeforeDirectoryRecheck == BeforeStageRecheck \union {RecheckDirectory}
BeforeTargetRecheck == BeforeDirectoryRecheck \union {RecheckTarget}

SubstituteStage(w) ==
    /\ Environment = ObservedChanges
    /\ edit.kind = NoEdit
    /\ attempts[w].phase \in BeforeStageRecheck
    /\ attempts' = [attempts EXCEPT ![w].stage = SubstitutedStage]
    /\ edit' = [kind |-> StageEdit, writer |-> w]
    /\ UNCHANGED <<lock, directory, target, history, cancelled, crashed>>

RedirectDirectory(w) ==
    /\ Environment = ObservedChanges
    /\ edit.kind = NoEdit
    /\ attempts[w].phase \in BeforeDirectoryRecheck
    /\ directory' = UnavailableDirectory
    /\ edit' = [kind |-> DirectoryEdit, writer |-> w]
    /\ UNCHANGED <<attempts, lock, target, history, cancelled, crashed>>

ChangeTarget(w, kind) ==
    /\ edit.kind = NoEdit
    /\ CASE kind = TargetEdit ->
               Environment = ObservedChanges /\ attempts[w].phase \in BeforeTargetRecheck
          [] kind = LateCreate ->
               Environment = ObservedChanges /\ At(w, Ready) /\ attempts[w].expectedTarget = Absent
          [] kind = LateOverwrite ->
               Environment = UntrustedLateChange /\ At(w, Ready) /\ attempts[w].expectedTarget # Absent
    /\ target' = ChangedTarget
    /\ edit' = [kind |-> kind, writer |-> w]
    /\ history' = Append(history, Change(FilesystemEdit, Absent, target, target', TRUE))
    /\ UNCHANGED <<attempts, lock, directory, cancelled, crashed>>

WriterStep(w) ==
    \/ AcquireDirectory(w)
    \/ RejectBusy(w)
    \/ CaptureTarget(w)
    \/ AuthorizeTarget(w)
    \/ CreateStage(w)
    \/ FinishWrite(w)
    \/ SyncStage(w)
    \/ RewindStage(w)
    \/ ValidateStage(w)
    \/ CheckStageIdentity(w)
    \/ CheckDirectoryIdentity(w)
    \/ CheckTargetStamp(w)
    \/ RequestCancel(w)
    \/ Abort(w)
    \/ Commit(w)
    \/ ReturnSuccess(w)
    \/ CrashProcess(w)
    \/ SubstituteStage(w)
    \/ RedirectDirectory(w)
    \/ ChangeTarget(w, TargetEdit)
    \/ ChangeTarget(w, LateCreate)
    \/ ChangeTarget(w, LateOverwrite)

Quiescent == /\ \A w \in Writers : attempts[w].phase \in Terminal
             /\ UNCHANGED vars

Next == (\E w \in Writers : WriterStep(w)) \/ Quiescent

TypeOk ==
    /\ attempts \in [Writers -> [phase : Phases, expectedDirectory : Directories,
                                 expectedTarget : Targets, authorized : BOOLEAN,
                                 stage : StageEntries, complete : BOOLEAN,
                                 synced : BOOLEAN, validated : BOOLEAN]]
    /\ lock \in Writers \union {Absent}
    /\ directory \in Directories
    /\ target \in Targets
    /\ edit \in [kind : EditKinds, writer : Writers \union {Absent}]
    /\ cancelled \in Writers \union {Absent}
    /\ crashed \in Writers \union {Absent}
    /\ Len(history) \in 1..4

ExclusivePublisher ==
    \A w \in Writers : (attempts[w].phase \in LockedPhases) = (lock = w)

AuthorizedPublication ==
    \A i \in 1..Len(history) : history[i].kind = Install => history[i].safe

AbortsPreserveOutput ==
    /\ target = history[Len(history)].after
    /\ \A i \in 2..Len(history) : history[i].before = history[i - 1].after
    /\ \A i \in 1..Len(history) :
        history[i].kind = Install => attempts[history[i].writer].phase \notin {Aborted, Busy}

ForeignStagePreserved ==
    edit.kind = StageEdit => attempts[edit.writer].stage = SubstitutedStage

=============================================================================
