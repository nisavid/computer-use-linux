# Domain Docs

This is a single-context repository.

## Before exploring

Read these when they exist:

- `CONTEXT.md` at the repository root.
- Relevant ADRs under `docs/adr/`.

If they do not exist, proceed silently. Create them lazily through
`domain-modeling` when terminology or an architectural decision is actually
resolved.

## Layout

```text
/
├── CONTEXT.md
├── docs/
│   └── adr/
└── src/
```

Use terminology defined in `CONTEXT.md`. Surface any conflict with an existing
ADR instead of silently overriding it.
