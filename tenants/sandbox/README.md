# The UniDPP sandbox tenant

This folder is a served tenant of the reference deployment (REARCH
R6): every request whose Host header names `sandbox.unidpp.org` is
answered from this folder's own module states, its own identity
journal, and its own branding, inside the one process.

The posture is stated, not implied: the sandbox carries no data
assurance of any kind. Its journals are demonstration state, may be
wiped at any time, and carry no commitment to retention, residency,
or availability. A passport minted in the sandbox is a demonstration
passport, and no party shall treat a sandbox verdict as a conformance
decision about a real product.

The public flip is an owner decision and remains deliberately untaken:
the tenant serves on loopback today, and `sandbox.unidpp.org` becomes
reachable publicly only when its DNS record and tunnel ingress are
provisioned. Until that word, the hostname is staged and inert.
