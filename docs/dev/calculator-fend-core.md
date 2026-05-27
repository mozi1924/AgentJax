# Calculator Engine (fend-core only)

## Current Status

As of 2026-05-28, AgentJax calculator is fully migrated to a single engine:

- runtime engine: `fend-core`
- native tool: `calculator`
- supported modes: `auto`, `evaluate`, `capabilities`

The previous symbolic stack and compatibility layer are removed from runtime and dependencies.

## Removed Surface

The following legacy symbolic modes/calls are intentionally removed:

- modes: `simplify`, `differentiate`, `integrate`, `solve`, `solve_system`, `limit`
- expression forms: `diff(...)`, `integral(...)`, `solve(...)`, `solve_system(...)`, `limit(...)`, `simplify(...)`, `factor(...)`, `expand(...)`

If these calls are sent by a model, calculator returns an explicit unsupported error.

## Why this change

- reduce tool ambiguity in new conversations
- avoid model drift/hallucination toward old symbolic APIs
- keep calculator behavior deterministic and easier to maintain

## Tool Schema Contract

`calculator` parameters now focus on fend-native evaluation only:

- `expression: string`
- `mode?: "auto" | "evaluate" | "capabilities"`
- `precision?: number`
- `variables?: object` (compiled into native fend assignments before evaluation)

No symbolic-mode parameters are exposed in schema.

## Maintenance Note

When updating prompts, examples, or tests, do not reintroduce symbolic-mode examples.
Use fend-native numeric/unit/complex expressions only.
Keep frontend and tool-layer normalization minimal so fend-core owns parsing,
precedence, and variable semantics wherever possible.
