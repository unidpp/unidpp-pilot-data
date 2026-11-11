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

The public flip was taken by the owner's decision on 2026-11-11: the
DNS record `sandbox.unidpp.org` (a proxied CNAME to the
`unidpp-sandbox` named tunnel) and the tunnel ingress (one hostname
to the deployment's origin) are live, and the tenant serves the
public alongside its five sibling aliases. Its tunnel connector rides
the same `one-up` loop as the rest of the estate
(`sandbox-tunnel.token` at the pilot root).
