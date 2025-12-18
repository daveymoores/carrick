# Carrick: project notes

This directory is intentionally minimal:

- `README.md` (this file)
- `OUTSTANDING_WORK.md` (what work remains)
- `research/` (reference docs and deeper analysis)
  - `next-steps/` (current direction documents)
  - `archive/` (historical documents kept for context; not current)

## What is Carrick?

Carrick is a GitHub Action / CLI that analyzes JavaScript/TypeScript repositories to:
- extract HTTP endpoints (producers) and outbound HTTP calls (consumers),
- match producers and consumers across repositories,
- report missing endpoints, type mismatches, and dependency conflicts.

## Current status (high level)

- Multi-agent extraction pipeline is in place (triage + specialist agents).
- Call sites include `context_slice` produced via single-file static slicing, and agents are instructed to use it.

See `OUTSTANDING_WORK.md` for the prioritized next steps.
