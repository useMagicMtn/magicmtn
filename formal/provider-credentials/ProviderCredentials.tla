---- MODULE ProviderCredentials ----
EXTENDS Naturals

CONSTANTS Connections, Accounts, Versions, MaxTicket

Owner(c) == c

NoCredential == [account |-> "none", version |-> "none", ticket |-> 0]
VARIABLES slots, pending, admitted

vars == <<slots, pending, admitted>>

Init ==
    /\ slots = [c \in Connections |-> [ticket |-> 0, revoked |-> FALSE, active |-> NoCredential]]
    /\ pending = {}
    /\ admitted = NoCredential

Begin(c, account, version) ==
    /\ ~slots[c].revoked
    /\ slots[c].ticket < MaxTicket
    /\ LET ticket == slots[c].ticket + 1 IN
       /\ slots' = [slots EXCEPT ![c].ticket = ticket]
       /\ pending' = pending \cup {
           [connection |-> c, account |-> account, version |-> version, ticket |-> ticket]}
    /\ UNCHANGED admitted

Finish(candidate) ==
    LET c == candidate.connection
        allowed == /\ candidate.ticket = slots[c].ticket
                   /\ candidate.account = Owner(c)
    IN /\ pending' = pending \ {candidate}
       /\ slots' = IF allowed THEN
                 [slots EXCEPT ![c].active = [account |-> candidate.account,
                    version |-> candidate.version, ticket |-> candidate.ticket]]
                    ELSE slots
       /\ UNCHANGED admitted

Revoke(c) ==
    /\ slots[c].ticket < MaxTicket
    /\ slots' = [slots EXCEPT ![c].ticket = @ + 1, ![c].revoked = TRUE, ![c].active = NoCredential]
    /\ UNCHANGED <<pending, admitted>>

Enable(c) ==
    /\ slots[c].revoked
    /\ slots' = [slots EXCEPT ![c].revoked = FALSE]
    /\ UNCHANGED <<pending, admitted>>

Admit(c) ==
    /\ slots[c].active # NoCredential
    /\ admitted' = slots[c].active
    /\ UNCHANGED <<pending, slots>>

Next ==
    \/ \E c \in Connections, account \in Accounts, version \in Versions : Begin(c, account, version)
    \/ \E candidate \in pending : Finish(candidate)
    \/ \E c \in Connections : Revoke(c) \/ Enable(c) \/ Admit(c)

Spec == Init /\ [][Next]_vars

LiveCredentialsMatchAccounts ==
    \A c \in Connections : slots[c].active = NoCredential \/ slots[c].active.account = Owner(c)

(* Publication must honor the most recent ticket and change only its own slot.
   Begin may leave a previously prepared credential available while refreshing. *)
PublicationFenced ==
    \A c \in Connections :
        slots'[c].active # slots[c].active /\ slots'[c].active # NoCredential
          => slots'[c].active.ticket = slots[c].ticket

RevokedCredentialsAreUnavailable ==
    \A c \in Connections : slots[c].revoked => slots[c].active = NoCredential

NoLatePublication == [][PublicationFenced]_vars

====
