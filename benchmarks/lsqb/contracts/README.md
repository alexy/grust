# Historical evidence contracts

These are byte-exact contracts for explicitly pinned historical source profiles,
not copies of today's canonical manifest. Do not format, update or replace them
when adding execution plans, host evidence or other current requirements.

The manifest named by SHA-256
`1dcae942840f216a83282f45f27e7fe228616e8f51af764689dc4f4fea0de849`
is 21,840 bytes. It is git blob `66e374bd07505e11ca4729d844addd2c71d5dbc6`
at both qualified SDK source revisions:

- Surreal SDK: `945dfa748b99e259f21973a1e86aaa339a028834`.
- Helix SDK3: `ed3febd88d35c5a6bd6c090787536dc0f33c85cd`.

`historical_contracts.py` admits only registered digests, reads regular
non-symlink files, hashes captured bytes before parsing, and rejects missing,
replaced or altered files. There is no runtime Git, output-directory or current
manifest fallback. New source profiles must explicitly select their own
authenticated contract and qualification requirements.

The historical manifest has no execution-plan registry or host-screen marker.
Their absence stays legacy/unknown; selecting this contract does not establish
current-source qualification, ongoing host isolation or native database speed.
Frozen bundle inventories and audit output schemas remain unchanged.
