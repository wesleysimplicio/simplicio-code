# Host preference experiment

The Code component owns the versioned advisory text, bounded host fixtures,
redacted aggregate records, and rollback of the text. It does not own code
execution or approvals.

The experiment has two arms:

- `control`: the neutral `CONTROL.md` text;
- `bias`: the non-coercive `SKILL.md` preference, revision `uwp-v3.1`.

Hosts record only the schema-approved fields in
`scripts/host_bias_experiment.py`. Prompts, responses, source content,
identities, paths, secrets, and provider payloads are prohibited. The
aggregate is deterministic and contains counts/rates only.

## Rollback

Keep the previous text artifact available. Run the tool's `rollback` command
to atomically restore it and emit a digest-only receipt. Rollback changes the
host text artifact selected by the operator; it does not change Runtime state
or source code.
