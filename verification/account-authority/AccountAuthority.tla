-------------------------- MODULE AccountAuthority --------------------------
EXTENDS Naturals, FiniteSets

CONSTANT SplitChecks
ASSUME SplitChecks \in BOOLEAN

Accounts == {"A", "B"}
Kinds == {"disable", "demote"}
Other(actor) == IF actor = "A" THEN "B" ELSE "A"

VARIABLES enabled, role, sessionLive, kind, stage, cachedStatus, result,
          usedRevokedAuthority
vars == <<enabled, role, sessionLive, kind, stage, cachedStatus, result,
          usedRevokedAuthority>>

Admins == {a \in Accounts : enabled[a] /\ role[a] = "administrator"}
Authorized(actor) == sessionLive[actor] /\ actor \in Admins
Decision(actor) ==
    IF ~Authorized(actor) THEN 401
    ELSE IF Cardinality(Admins) <= 1 THEN 409
    ELSE 204

Init ==
    /\ enabled = [a \in Accounts |-> TRUE]
    /\ role = [a \in Accounts |-> "administrator"]
    /\ sessionLive = [a \in Accounts |-> TRUE]
    /\ kind \in [Accounts -> Kinds]
    /\ stage = [a \in Accounts |-> "pending"]
    /\ cachedStatus = [a \in Accounts |-> 0]
    /\ result = [a \in Accounts |-> 0]
    /\ usedRevokedAuthority = FALSE

\* One successful transaction changes the target and invalidates its session.
Finish(actor, status) ==
    LET target == Other(actor)
        success == status = 204
    IN
    /\ enabled' = IF success /\ kind[actor] = "disable"
                   THEN [enabled EXCEPT ![target] = FALSE] ELSE enabled
    /\ role' = IF success /\ kind[actor] = "demote"
                THEN [role EXCEPT ![target] = "user"] ELSE role
    /\ sessionLive' = IF success
                       THEN [sessionLive EXCEPT ![target] = FALSE]
                       ELSE sessionLive
    /\ result' = [result EXCEPT ![actor] = status]
    /\ stage' = [stage EXCEPT ![actor] = "done"]
    /\ usedRevokedAuthority' = (usedRevokedAuthority
                                \/ (success /\ ~Authorized(actor)))
    /\ UNCHANGED <<kind, cachedStatus>>

\* Current source: authorization, count guard and effect share a transaction.
Execute(actor) ==
    /\ ~SplitChecks
    /\ stage[actor] = "pending"
    /\ Finish(actor, Decision(actor))

\* Negative control: cache BOTH authorization and count before applying.
SnapshotChecks(actor) ==
    /\ SplitChecks
    /\ stage[actor] = "pending"
    /\ cachedStatus' = [cachedStatus EXCEPT ![actor] = Decision(actor)]
    /\ stage' = [stage EXCEPT ![actor] = "checked"]
    /\ UNCHANGED <<enabled, role, sessionLive, kind, result,
                   usedRevokedAuthority>>

ApplyCached(actor) ==
    /\ SplitChecks
    /\ stage[actor] = "checked"
    /\ Finish(actor, cachedStatus[actor])

Done ==
    /\ \A actor \in Accounts : stage[actor] = "done"
    /\ UNCHANGED vars

Next == (\E actor \in Accounts : Execute(actor))
        \/ (\E actor \in Accounts : SnapshotChecks(actor))
        \/ (\E actor \in Accounts : ApplyCached(actor))
        \/ Done

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ enabled \in [Accounts -> BOOLEAN]
    /\ role \in [Accounts -> {"administrator", "user"}]
    /\ sessionLive \in [Accounts -> BOOLEAN]
    /\ kind \in [Accounts -> Kinds]
    /\ stage \in [Accounts -> {"pending", "checked", "done"}]
    /\ cachedStatus \in [Accounts -> {0, 204, 401, 409}]
    /\ result \in [Accounts -> {0, 204, 401, 409}]
    /\ usedRevokedAuthority \in BOOLEAN

AtLeastOneAdmin == Cardinality(Admins) >= 1
NoSuccessfulRevokedAuthority == ~usedRevokedAuthority
CompletedRaceOutcome ==
    (\A actor \in Accounts : stage[actor] = "done") =>
      /\ Cardinality(Admins) = 1
      /\ {result[a] : a \in Accounts} = {204, 401}
=============================================================================
