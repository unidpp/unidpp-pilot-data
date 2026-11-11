# The UniDPP sandbox tenant

This folder is the demo tenant of the deployment (REARCH R6): every
request whose Host header names `demo.unidpp.org` is answered from
this folder's own module states, its own identity journal, and its
own branding, inside the one process. The folder's id is still
`sandbox` in history; the public name is demo.

The posture is stated, not implied: the sandbox carries no data
assurance of any kind. Its journals are demonstration state, may be
wiped at any time, and carry no commitment to retention, residency,
or availability. A passport minted in the sandbox is a demonstration
passport, and no party shall treat a sandbox verdict as a conformance
decision about a real product.

The public flip was taken by the owner's decision on 2026-11-11, and
the owner named the public surface `demo.unidpp.org` (a proxied CNAME
to the `unidpp-demo` named tunnel); the tunnel connector rides the
same `one-up` loop as the rest of the estate (`demo-tunnel.token` at
the pilot root). The production origin is the remote host (R9 step
2): the connectors run on it, not on the workstation.
