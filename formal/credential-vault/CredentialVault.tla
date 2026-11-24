---- MODULE CredentialVault ----
EXTENDS Naturals

CONSTANTS Workers, MaxVersion

(* One ID, two workers, and a representative mismatch for each scope field.
   Each row transition represents an atomic SQL statement. A read represents
   snapshot selection; decrypting a snapshot already returned may finish later. *)
OrganizationScope ==
    [org |-> "org-a", owner |-> "organization", connection |-> "connection-a",
     purpose |-> "provider-modal"]

PersonalScope == [OrganizationScope EXCEPT !.owner = "owner-a"]

Scopes == {OrganizationScope, PersonalScope,
           [OrganizationScope EXCEPT !.org = "org-b"],
           [PersonalScope EXCEPT !.owner = "owner-b"],
           [OrganizationScope EXCEPT !.connection = "connection-b"],
           [OrganizationScope EXCEPT !.purpose = "provider-baseten"]}

Mutations == {"replace", "destroy"}
Versions == 1..MaxVersion
EmptyRow == [status |-> "absent", scope |-> OrganizationScope, version |-> 0,
             hasEnvelope |-> FALSE]

Idle == [kind |-> "idle", scope |-> OrganizationScope, expected |-> 0]
NoEvent == [kind |-> "none", scope |-> OrganizationScope, before |-> EmptyRow,
            expected |-> 0]

VARIABLES row, pending, event, everDestroyed

vars == <<row, pending, event, everDestroyed>>

Init ==
    /\ row = EmptyRow
    /\ pending = [w \in Workers |-> Idle]
    /\ event = NoEvent
    /\ everDestroyed = FALSE

Matches(s) ==
    /\ s.org = row.scope.org
    /\ s.owner = row.scope.owner
    /\ s.connection = row.scope.connection
    /\ s.purpose = row.scope.purpose

LiveMatch(s) == row.status = "live" /\ Matches(s)

Create(s) ==
    /\ row.status = "absent"
    /\ row' = [status |-> "live", scope |-> s, version |-> 1,
                hasEnvelope |-> TRUE]
    /\ event' = NoEvent
    /\ UNCHANGED <<pending, everDestroyed>>

(* Read and metadata use the same SQL predicate. Rejected lookups leave no state. *)
Read(s) ==
    /\ LiveMatch(s)
    /\ event' = [kind |-> "read", scope |-> s, before |-> row, expected |-> 0]
    /\ UNCHANGED <<row, pending, everDestroyed>>

(* The preflight lookup and final UPDATE can be interleaved with another writer. *)
Begin(w, s, kind) ==
    /\ pending[w] = Idle
    /\ LiveMatch(s)
    /\ row.version < MaxVersion
    /\ pending' = [pending EXCEPT ![w] = [kind |-> kind, scope |-> s,
                                          expected |-> row.version]]
    /\ event' = NoEvent
    /\ UNCHANGED <<row, everDestroyed>>

Finish(w) ==
    /\ pending[w] # Idle
    /\ LET request == pending[w]
           accepts == LiveMatch(request.scope) /\ row.version = request.expected
           destroys == request.kind = "destroy"
       IN /\ pending' = [pending EXCEPT ![w] = Idle]
          /\ row' = IF accepts
                     THEN [row EXCEPT
                            !.status = IF destroys THEN "destroyed" ELSE "live",
                            !.version = request.expected + 1,
                            !.hasEnvelope = ~destroys]
                     ELSE row
          /\ event' = IF accepts
                       THEN [kind |-> request.kind, scope |-> request.scope,
                             before |-> row, expected |-> request.expected]
                       ELSE NoEvent
          /\ everDestroyed' = (everDestroyed \/ (accepts /\ destroys))

Next ==
    \/ \E s \in Scopes : Create(s) \/ Read(s)
    \/ \E w \in Workers, s \in Scopes, kind \in Mutations : Begin(w, s, kind)
    \/ \E w \in Workers : Finish(w)

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ row \in [status : {"absent", "live", "destroyed"}, scope : Scopes,
                 version : 0..MaxVersion, hasEnvelope : BOOLEAN]
    /\ pending \in [Workers -> ({Idle} \cup
                      [kind : Mutations, scope : Scopes, expected : Versions])]
    /\ everDestroyed \in BOOLEAN

ReadsRequireLiveExactScope ==
    IF event = NoEvent THEN TRUE
    ELSE event.kind = "read" =>
         (event.before.status = "live" /\ event.scope = event.before.scope)

WritesRequireLiveExactScope ==
    IF event = NoEvent THEN TRUE
    ELSE event.kind \in Mutations =>
         (event.before.status = "live" /\ event.scope = event.before.scope)

WritesRequireCurrentVersion ==
    IF event = NoEvent THEN TRUE
    ELSE event.kind \in Mutations =>
         (event.expected = event.before.version /\ row.version = event.before.version + 1)

DestroyedIdsAreNeverReused ==
    everDestroyed => row.status = "destroyed"

OnlyLiveRowsHaveAnEnvelope ==
    row.hasEnvelope <=> row.status = "live"

====
