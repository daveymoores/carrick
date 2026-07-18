# Fix: require the response-locator fields in the file-analyzer endpoint schema

**Type:** scanner-only, no cloud deploy. **Blast radius:** one line + tests. **Target metric:**
cross-repo type **resolution** (~0.73 → higher), without regressing endpoint / cross-repo-match F1.

## The change (one line)

In `src/agents/schemas.rs`, function `file_analysis_schema()`, the **endpoint** `items`
`required` array (currently **line 335**) — append `response_expression_text` and
`response_expression_line`:

```rust
// before
"required": ["candidate_id", "line_number", "owner_node", "method", "path", "handler_name", "pattern_matched", "emission_style", "payload_expression_text", "payload_expression_line"]

// after
"required": ["candidate_id", "line_number", "owner_node", "method", "path", "handler_name", "pattern_matched", "emission_style", "payload_expression_text", "payload_expression_line", "response_expression_text", "response_expression_line"]
```

**Do NOT** add `primary_type_symbol` or `type_import_source` to `required` — leave them
nullable/optional (see "What not to touch").

No `system_prompt.txt` change and no lambda redeploy: the scanner sends this schema in the
`POST /analyze-file` request body and the deployed file-analyzer lambda validates + forwards it
to Gemini unchanged. The change takes effect from the scanner build. The prompt already
instructs the model to emit these fields ("MUST emit ..."), so this only makes the schema
enforce what the prompt already asks.

## Why

Under Gemini structured output, the model is only *forced* to emit fields in the `required`
list. The response locators are currently optional, so `gemini-3.1-flash-lite` **silently omits
`response_expression_line`** — even when it correctly identified the response expression. Without
that line, the ts-morph type sidecar has no anchor to infer the response type at, so the response
contract resolves to `unknown`. This is a direct, reproducible contributor to the resolution
ceiling. The model reads line numbers off the per-line prefixes in the file content (it does not
count), so once the field is required it emits the correct number it already has.

## Evidence (live `gemini-3.1-flash-lite`, temp 0, thinking=low, 3 runs each, via the offline prompt-harness)

| fixture | current schema (locators optional) | with locators required |
|---|---|---|
| `named-handler-cast` (handler near route) | drops `response_expression_line` | emits correctly, 3/3 |
| `no-payload-mixed` (204 / no body) | drops field | correctly emits **null** — no hallucinated line |
| `named-handler-distance` (handler 288 lines above its route + same-text `res.json(order)` distractors) | drops field | correct far-up line (12), 3/3 — **no distractor grab, no hallucination** |

One change covers three things at once: field **presence**, **no-payload safety** (nullable
locator correctly stays null), and **distance** (the model localizes across ~290 lines fine — it
only needed forcing to *emit*). No handler-location-linking mechanism is needed.

## What not to touch

- **`primary_type_symbol` / `type_import_source` stay optional.** Tested: when forced required,
  the model borrows the *wrong* type (the request-body type) on inline-object responses where the
  correct answer is `null`. The response locator alone is sufficient — the sidecar resolves the
  type from the location ("AI locates, ts-morph resolves"). Forcing the symbol adds risk for no
  needed benefit.
- **Line numbers / prefixes** — already reliable (read off prefixes). Do not change.

## Tests

Run the scanner test suite. The two tests that read the endpoint `required` array —
`emission_style_schema_enum_matches_serde_wire_values` (asserts it *contains* `emission_style`
via `.any()`) and `test_file_analysis_schema_structure` (property-existence checks) — do **not**
assert exact array contents, so they should stay green. If any snapshot/cassette test asserts the
exact request schema, update it to include the two new fields. No serde or prompt change needed.

## Validate on the eval

Run `eval-xrepo` on `xrepo-corpus-1` and `xrepo-corpus-2` (e.g. `runs=5`), before and after:

- **Primary signal:** the type-**resolution** dimension should rise (more response types resolved
  because the anchor is now consistently present).
- **Guardrail (must not regress):** endpoint-set P/R/F1 and cross-repo match P/R/F1 (both ~1.0 on
  corpus-1) hold.
- Resolution is **stochastic** — compare **per-edge** using the `ts_check` output artifacts, not
  the noisy run mean. Treat a single run as directional; confirm with the multi-run sample.

**Rollback:** revert the one-line `required`-array change. Zero blast radius otherwise.

## Regression fixtures (already authored)

In `carrick-cloud/lambdas/file-analyzer/prompt-harness/fixtures/`:
- `no-payload-mixed` — has-payload GET + no-payload DELETE; guards the nullable-locator case.
- `named-handler-distance` — handler 288 lines from its route + distractors; guards the
  distance/localization case.

Keep both as harness regression fixtures for this behaviour.

## Separate follow-up (carrick-cloud repo, optional, not required for this fix)

The offline harness snapshot `carrick-cloud/lambdas/file-analyzer/prompt-harness/response_schema.json`
has drifted from `schemas.rs` (it under-requires even the payload fields), so iterating prompts
against it can mislead. Resync it to `file_analysis_schema()` and add a cross-repo sync check.
