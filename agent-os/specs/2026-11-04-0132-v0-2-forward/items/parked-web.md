# Parked — Web compilation (JS/HTML/CSS)

**Status: parked. Do not spec, do not implement.**

The design doc parks the web target explicitly. The only required
guardrail for this milestone:

When F1 (GUI pilot) lands, keep its notes about what a portable widget
layer would need — the same widget tree that would render to a GPU
surface renders to DOM later. Any design choice in F1 that would make
that mapping *impossible* is a red flag worth surfacing, not blocking.

Re-open this item only after the GUI story exists (path b in the
design doc's own ordering).
