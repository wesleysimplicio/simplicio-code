# Loop Hub client contract

`scripts/validate_loop_hub_client.py` emits
`simplicio.loop-hub-client/v1`. It makes the ownership decision explicit for
Code: with a ready Hub, Runtime, Mapper, scheduler, and inference capacity have
one Hub owner and Code reuses the handles; `standalone` is an explicit mode,
not a silent fallback.

The contract prevents duplicate daemon declarations and provides the admission
gate needed by issue #55. It does not claim that the external Loop Hub daemon
or transport adapter is implemented in this repository yet.
