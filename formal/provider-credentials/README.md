# Provider credential publication

Run `python3 scripts/check-provider-credentials.py --mutation-checks`. The model checks two connection slots, two account identities, two versions, and overlapping preparation attempts. Revocation and newer preparation attempts fence older publications. A successful publication matches the connection's pinned account. Dispatch observes the active snapshot at admission.

The model represents the atomic in-memory slot updates in the provider host. Account identity comes from the trusted adapter and the durable connection binding. The local integration test exercises storage, subprocess startup, and command dispatch.
