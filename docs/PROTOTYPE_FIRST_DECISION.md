# Prototype-First decision gate

`scripts/validate_prototype_decision.py` emits
`simplicio.prototype-decision/v1`. A decision receipt names the plan and source
revision, describes one or more typed artifacts, records assumptions and
limitations, and accepts only `accept`, `revise`, or `reject`.

Build authorization is fail-closed: it requires a current `accept` receipt;
source drift, malformed or malicious artifact references, missing evidence,
`revise`, and `reject` all block Build. The same receipt can be rendered by
TUI, headless, ACP, or workspace surfaces. This PR supplies the decision
contract/gate; the full visual gallery and external Loop #568 integration still
need their owning surfaces.
