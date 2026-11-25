--------------------------- MODULE StagedRelease ---------------------------
EXTENDS Naturals

CONSTANT Mutation

Production == "production"
ReuseStaleTicket == "reuse stale ticket"
RetryAmbiguousUpload == "retry ambiguous upload"

ASSUME Mutation \in {Production, ReuseStaleTicket, RetryAmbiguousUpload}

(* A digest token stands for exact image bytes and length. The revision is
   the sealed volume payload. Signing and stapling only change the envelope. *)
Revisions == 1..2
SubmissionIds == 1..2
NoSubmission == 0
Unsigned == "unsigned"
Signed == "signed"
Stapled == "stapled"
NoImage == [revision |-> 0, envelope |-> "absent"]
Image(revision, envelope) == [revision |-> revision, envelope |-> envelope]
Images(envelope) == {Image(r, envelope) : r \in Revisions}
AnyImage == Images(Unsigned) \cup Images(Signed) \cup Images(Stapled)
MaybeImage == AnyImage \cup {NoImage}

Plans ==
	{p \in [apps : BOOLEAN, sign : BOOLEAN, notary : BOOLEAN,
	         staple : BOOLEAN, assess : BOOLEAN] :
		/\ p.notary => p.sign
		/\ p.staple => p.notary
		/\ p.assess => p.sign}

PrepareApps == "prepare apps"
Build == "build"
CheckUnsigned == "check unsigned"
CheckApps == "check assembled apps"
SignImage == "sign image"
Reserve == "reserve submission"
Upload == "submit"
Wait == "wait for notary"
StapleImage == "staple image"
CheckFinal == "check final"
Assess == "assess policy"
Publish == "publish"
Complete == "complete"
Failed == "failed"
Cancelled == "cancelled"

Terminal == {Complete, Failed, Cancelled}
Phases ==
	{PrepareApps, Build, CheckUnsigned, CheckApps, SignImage, Reserve,
	 Upload, Wait, StapleImage, CheckFinal, Assess, Publish} \cup Terminal

NoStatus == "no status"
Pending == "pending"
Unknown == "unknown"
Accepted == "accepted"
Rejected == "rejected"
Statuses == {NoStatus, Pending, Unknown, Accepted, Rejected}

VARIABLE s

Init ==
	\E plan \in Plans, revision \in Revisions :
		s = [
			plan |-> plan,
			phase |-> IF plan.apps THEN PrepareApps ELSE Build,
			captured |-> revision,
			source |-> revision,
			sourceProof |-> 0,
			unsigned |-> NoImage,
			artifact |-> NoImage,
			observed |-> NoImage,
			replaced |-> FALSE,
			unsignedProof |-> NoImage,
			appsProof |-> NoImage,
			signInput |-> NoImage,
			signed |-> NoImage,
			reservation |-> NoImage,
			uploads |-> 0,
			id |-> NoSubmission,
			notaryStatus |-> NoStatus,
			terminalDigest |-> NoImage,
			ticketInput |-> NoImage,
			ticketId |-> NoSubmission,
			finalProof |-> NoImage,
			assessment |-> NoImage,
			publication |-> NoImage
		]

At(phase) == s.phase = phase
Live == s.phase \notin Terminal
SameSource == s.source = s.captured
Bound == s.artifact # NoImage /\ s.observed = s.artifact
NextAppsOrSign ==
	IF s.plan.apps THEN CheckApps ELSE SignImage
AppsReady ==
	~s.plan.apps \/
		(s.sourceProof = s.captured /\ s.appsProof # NoImage /\ s.appsProof = s.unsigned)

Prepare ==
	/\ At(PrepareApps)
	/\ SameSource
	/\ s' = [s EXCEPT !.sourceProof = s.captured, !.phase = Build]

BuildImage ==
	/\ At(Build)
	/\ SameSource
	/\ ~s.plan.apps \/ s.sourceProof = s.captured
	/\ LET image == Image(s.captured, Unsigned)
	   IN s' = [s EXCEPT
			!.unsigned = image, !.artifact = image, !.observed = image,
			!.phase = IF s.plan.sign THEN CheckUnsigned
			          ELSE IF s.plan.apps THEN CheckApps ELSE CheckFinal]

VerifyUnsigned ==
	/\ At(CheckUnsigned)
	/\ Bound /\ s.artifact = s.unsigned
	/\ s' = [s EXCEPT !.unsignedProof = s.observed, !.phase = NextAppsOrSign]

VerifyApps ==
	/\ At(CheckApps)
	/\ Bound /\ s.artifact = s.unsigned
	/\ SameSource
	/\ s.sourceProof = s.captured
	/\ s' = [s EXCEPT
		!.appsProof = s.observed,
		!.phase = IF s.plan.sign THEN SignImage ELSE CheckFinal]

Sign ==
	/\ At(SignImage)
	/\ SameSource /\ AppsReady
	/\ Bound /\ s.artifact = s.unsigned /\ s.unsignedProof = s.unsigned
	/\ LET image == Image(s.observed.revision, Signed)
	   IN s' = [s EXCEPT
			!.signInput = s.observed,
			!.signed = image, !.artifact = image, !.observed = image,
			!.phase = IF s.plan.notary THEN Reserve ELSE CheckFinal]

ReserveSubmission ==
	/\ At(Reserve)
	/\ SameSource /\ Bound /\ s.artifact = s.signed
	/\ s.reservation = NoImage
	/\ s' = [s EXCEPT !.reservation = s.observed, !.phase = Upload]

Submit(id, known) ==
	/\ \/ At(Upload) /\ s.uploads = 0
	   \/ /\ Mutation = RetryAmbiguousUpload
	      /\ At(Wait) /\ s.notaryStatus = Unknown /\ s.id = NoSubmission
	      /\ s.uploads = 1
	/\ Bound /\ s.observed = s.reservation
	/\ s' = [s EXCEPT
		!.uploads = @ + 1,
		!.id = IF known THEN id ELSE NoSubmission,
		!.notaryStatus = IF known THEN Pending ELSE Unknown,
		!.phase = Wait]

(* A terminal outcome needs an exact digest-bound log for the queried ID.
   A previously known ID also binds resume. An ID can be recovered from an
   ambiguous reservation only when its terminal log matches that digest. *)
ReceiveTerminal(queried, reported, digest, outcome) ==
	/\ At(Wait) /\ Bound /\ s.observed = s.reservation
	/\ s.id = NoSubmission \/ s.id = queried
	/\ reported = queried /\ digest = s.reservation
	/\ s' = [s EXCEPT
		!.id = queried, !.notaryStatus = outcome, !.terminalDigest = digest,
		!.phase = IF outcome = Rejected THEN Failed
		          ELSE IF s.plan.staple THEN StapleImage ELSE CheckFinal]

UnknownPoll ==
	/\ At(Wait) /\ Bound /\ s.observed = s.reservation
	/\ s.notaryStatus = Pending
	/\ s' = [s EXCEPT !.notaryStatus = Unknown]

PendingPoll ==
	/\ At(Wait) /\ Bound /\ s.observed = s.reservation
	/\ s.id # NoSubmission /\ s.notaryStatus = Unknown
	/\ s' = [s EXCEPT !.notaryStatus = Pending]

Staple ==
	/\ At(StapleImage)
	/\ s.notaryStatus = Accepted /\ s.id # NoSubmission
	/\ s.observed \in Images(Signed)
	/\ \/ Bound /\ s.observed = s.terminalDigest
	   \/ Mutation = ReuseStaleTicket
	/\ LET image == Image(s.observed.revision, Stapled)
	   IN s' = [s EXCEPT
			!.ticketInput = s.observed, !.ticketId = s.id,
			!.artifact = image, !.observed = image, !.phase = CheckFinal]

VerifyFinal ==
	/\ At(CheckFinal) /\ Bound
	/\ s' = [s EXCEPT
		!.finalProof = s.observed,
		!.phase = IF s.plan.assess THEN Assess ELSE Publish]

AssessFinal ==
	/\ At(Assess) /\ Bound /\ s.finalProof = s.observed
	/\ s' = [s EXCEPT !.assessment = s.observed, !.phase = Publish]

PublishImage ==
	/\ At(Publish) /\ Bound /\ SameSource
	/\ s' = [s EXCEPT !.publication = s.observed, !.phase = Complete]

ChangeSource ==
	/\ Live /\ SameSource
	/\ \E revision \in Revisions \ {s.source} :
		s' = [s EXCEPT !.source = revision]

ReplaceArtifact ==
	/\ Live /\ Bound /\ ~s.replaced
	/\ \E replacement \in AnyImage \ {s.observed} :
		s' = [s EXCEPT !.observed = replacement, !.replaced = TRUE]

Fail == Live /\ s' = [s EXCEPT !.phase = Failed]
Cancel == Live /\ s' = [s EXCEPT !.phase = Cancelled]
Stop == ~Live /\ UNCHANGED s

Next ==
	\/ Prepare \/ BuildImage \/ VerifyUnsigned \/ VerifyApps \/ Sign
	\/ ReserveSubmission
	\/ \E id \in SubmissionIds, known \in BOOLEAN : Submit(id, known)
	\/ \E queried, reported \in SubmissionIds, digest \in Images(Signed),
	      outcome \in {Accepted, Rejected} :
		ReceiveTerminal(queried, reported, digest, outcome)
	\/ UnknownPoll \/ PendingPoll
	\/ Staple \/ VerifyFinal \/ AssessFinal \/ PublishImage
	\/ ChangeSource \/ ReplaceArtifact \/ Fail \/ Cancel \/ Stop

TypeOk ==
	s \in [
		plan : Plans,
		phase : Phases,
		captured : Revisions,
		source : Revisions,
		sourceProof : Revisions \cup {0},
		unsigned : Images(Unsigned) \cup {NoImage},
		artifact : MaybeImage,
		observed : MaybeImage,
		replaced : BOOLEAN,
		unsignedProof : Images(Unsigned) \cup {NoImage},
		appsProof : Images(Unsigned) \cup {NoImage},
		signInput : Images(Unsigned) \cup {NoImage},
		signed : Images(Signed) \cup {NoImage},
		reservation : Images(Signed) \cup {NoImage},
		uploads : 0..2,
		id : SubmissionIds \cup {NoSubmission},
		notaryStatus : Statuses,
		terminalDigest : Images(Signed) \cup {NoImage},
		ticketInput : Images(Signed) \cup {NoImage},
		ticketId : SubmissionIds \cup {NoSubmission},
		finalProof : MaybeImage,
		assessment : MaybeImage,
		publication : MaybeImage
	]

AppEvidenceBound ==
	s.appsProof # NoImage =>
		/\ s.plan.apps /\ s.sourceProof = s.captured
		/\ s.appsProof = s.unsigned

SigningAuthorized ==
	s.signed # NoImage =>
		/\ s.plan.sign /\ AppsReady
		/\ s.signInput = s.unsigned /\ s.unsignedProof = s.signInput
		/\ s.signed = Image(s.signInput.revision, Signed)

SingleSubmission == s.uploads <= 1

NotarizationBound ==
	/\ s.uploads > 0 =>
		/\ s.plan.notary /\ s.reservation # NoImage
		/\ s.reservation = s.signed
	/\ s.notaryStatus \in {Accepted, Rejected} =>
		/\ s.uploads > 0 /\ s.id # NoSubmission
		/\ s.terminalDigest = s.reservation

StaplingAuthorized ==
	s.ticketInput # NoImage =>
		/\ s.plan.staple /\ s.notaryStatus = Accepted
		/\ s.ticketInput = s.terminalDigest /\ s.ticketId = s.id
		/\ s.artifact = Image(s.ticketInput.revision, Stapled)

ExpectedFinal ==
	IF s.plan.staple THEN Image(s.ticketInput.revision, Stapled)
	ELSE IF s.plan.sign THEN s.signed ELSE s.unsigned

PublicationAuthorized ==
	s.publication # NoImage =>
		/\ At(Complete) /\ SameSource /\ AppsReady
		/\ s.publication = ExpectedFinal /\ s.publication = s.artifact
		/\ s.publication = s.finalProof
		/\ ~s.plan.sign \/ s.signed # NoImage
		/\ ~s.plan.notary \/ s.notaryStatus = Accepted
		/\ ~s.plan.staple \/ s.ticketInput # NoImage
		/\ ~s.plan.assess \/ s.assessment = s.publication

=============================================================================
