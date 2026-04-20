# Carrick Growth Playbook

The goal: drive real signups and usage traction before the VC demo window closes. This is the consolidated plan.

## Honest baseline

- Prior blog post: 3k views → 2 signups (0.07%). The old site was the bottleneck; new site should do 1–3% on the same audience.
- Current blocker is **friction**, not just traffic. Waitlist-only + no demo video + no way to try = dead funnel on an HN launch.
- Demo video is the single highest-leverage unlock.
- The "closed source protects the moat" frame is misleading: the scanner is the hard engineering, but standalone it's incomplete. The commercial moat is the hosted backend (cross-repo data gravity, billing), not the scanner code.

## The core unlock: repo split + self-serve GitHub Action

This is the change that makes everything else work. Without it you can't do the OSS teardown posts, can't launch on HN without a "another waitlist" backlash, and can't convert HN traffic at scale.

The earlier version of this doc proposed a local-only `npx carrick` tier to make HN "try it now" frictionless. That's been cut. Carrick's value prop is **cross-repo drift** — a local single-repo scan sells the weakest version of the product, muddies the messaging ("is this a CLI or a CI tool?"), and cuts you off from the user feedback loop exactly when you need it most at MVP. The GitHub Action is already the production install path; leaning into it keeps the product story coherent.

### Repo split

**New public repo** — `carrick` (Elastic License 2.0, source-available):
- `src/` — Rust scanner, framework detector, agents, call-site classifier
- `ts_check/` — TypeScript type-checking sidecar
- `mcp-server/` — client-facing MCP piece
- `action.yml` — GitHub Action entry point

**New private repo** — `carrick-cloud` (private GitHub repo, proprietary, no public license):
- `lambdas/` — agent-proxy, check-or-upload, mcp-server lambda, file-analyzer, framework-guidance
- `terraform/` — all AWS infra
- `app/` — web dashboard + OAuth + service-map pages (new; see §product surface below)
- Billing, org management, API-key issuance
- **All prompts, LLM-facing schema descriptions, and rule-generation logic live here.** See `.thoughts/public-private-split.md` for the boundary details.

ELv2 explicitly prevents anyone hosting it as a competing SaaS. Sentry, PostHog, MinIO, and Elastic itself use this model — it's the default source-available license for VC-backed dev tools, reads cleanly in diligence, and answers "what stops AWS from cloning you?" in one sentence. Framed publicly as "source-available, not open source."

### Self-serve onboarding (replaces the waitlist)

No local CLI. No anonymous tier. No email waitlist. The flow is:

1. Land on `carrick.tools`, watch the demo video, click **"Sign in with GitHub"**.
2. OAuth completes → API key minted automatically, shown once with copy button.
3. Dashboard displays a prefilled `.github/workflows/carrick.yml` snippet + secret setup instructions.
4. User commits the workflow → next PR gets a Carrick comment with drift findings + a link to the hosted service map.

Signup-to-first-scan should feel like 60 seconds of work. The friction HN hates isn't "install an action" — it's "we'll email you when you're approved."

### Tiers

| Tier | Who | Limits | Access |
|---|---|---|---|
| **Free** (GitHub OAuth, instant) | Anyone who signs up | Per-repo daily cap; global circuit breaker at ~$800 MTD | Single-repo PR scans, PR comments, shareable service map mirroring repo visibility |
| **Cross-repo** (one-click request) | Users who click "Request cross-repo access" from their dashboard | Negotiated on the intro call | Multi-repo uploads, cross-service drift analysis, MCP queries |
| **Paid** | Post-intro customers | Unlimited | Full feature set, SLA, private support |

**No application form for cross-repo access** — one button, the request hits your inbox / Slack, you reach out and run the intro call. The call is the filter, not the form. A form filters out exactly the curious-but-serious users you want to talk to.

**Cost ceiling: $1k/month.** At ~$0.20/scan average that's ~5,000 scans. Size per-key limits so aggregate spend can't blow past the circuit breaker.

**Why GitHub OAuth is load-bearing (not just a signup gate):**
- Identity → ownership of service maps and shareable URLs.
- Verified email + username + org memberships → better signal than any form, and pre-wires the paid-conversion path.
- Enables per-viewer permission checks for private-repo map pages (see §product surface).
- Every PR scan can link to a `carrick.tools/<owner>/<repo>/map` page — latent viral loop as coworkers click through.

### Scanner-side gating

- The scanner only runs from the GitHub Action with `CARRICK_API_KEY` set. No unkeyed code path.
- Backend classifies the key's tier on every request: free (single-repo) vs cross-repo vs paid.
- Free-tier keys: `cloud_storage/` uploads and cross-repo graph builds are rejected server-side. The PR comment includes a CTA: "Cross-repo drift requires access — request at carrick.tools/request".
- Cross-repo keys: full pipeline.

### Product surface to build

Net-new pieces of `carrick-cloud`. This is the work-of-record under Option C.

**Full details on GitHub OAuth, API key issuance, map access control, data model, and phase ordering live in `.thoughts/github-oauth.md`.** Read that before building any of the auth/identity layer. Summary:

1. **GitHub OAuth app.** Standard flow. Scopes: `read:user`, `read:org`, `public_repo` at signup; request `repo` (private repo read) only when the user opens a private-repo map page, so private-repo access is opt-in per-use rather than upfront.
2. **API key issuance.** Mint on first sign-in; show once; allow rotation from dashboard. Keys are tier-scoped server-side.
3. **Dashboard** at `app.carrick.tools` (or `carrick.tools/app`):
   - Prefilled Action snippet with the user's key wired to a secret placeholder + step-by-step copy-paste instructions.
   - List of repos that have reported scans (auto-populated from incoming Action runs).
   - Links to each repo's service map.
   - Single "Request cross-repo access" button → email to you, flag toggled on approval.
4. **Service map pages** at `carrick.tools/<owner>/<repo>/map`:
   - Render service graph + endpoint list + drift findings.
   - **Visibility mirrors the GitHub repo.** Captured at upload time from `GITHUB_REPOSITORY` + GitHub API repo metadata, stored with the map, re-verified on each read.
   - Private-repo maps require the viewer to authenticate and pass a GitHub API read-access check (collaborator or org member) on every page load. Cache the check briefly (5 min) to avoid hammering GitHub's API.
   - Public-repo maps are ungated public URLs.
5. **PR comment template** — include the shareable map URL alongside the drift findings block.
6. **Admin surface for cross-repo approvals.** Lightweight — email notification + a boolean in an admin-only page that flips the user's tier. No full admin panel needed at MVP.

### Public / private boundary

**Rule: Rust = public (`carrick`, ELv2). Lambdas = private (`carrick-cloud`, proprietary).** Mechanical split, no case-by-case debates. Anything worth protecting (prompts, rule generation) gets rewritten as lambda code in `carrick-cloud`. Everything in the Rust scanner stays public in `carrick`.

Details — what moves, what stays, schema ownership, protocol shape, contract versioning, migration plan — live in the dedicated doc: `.thoughts/public-private-split.md`. Read that before touching anything in `src/agents/`.

Summary of what moves out of Rust: prompt strings in `src/agents/*`, schema descriptions in `src/agents/schemas.rs`, and wrapper rule generation in `src/wrapper_registry.rs`. Everything else (scanner, mount graph, analyzer, sidecar management, orchestrator plumbing) stays in Rust.

### Infra work on the proxy

Simpler than the earlier plan since there's no anonymous tier to defend:
- **Per-key** rate limiting (keyed on API key, not IP)
- Global circuit breaker at ~$800 MTD spend with graceful degradation
- Repo-size / file-count cap enforced before accepting a scan
- Per-call token cap so one pathological file can't burn $5
- Tier check on every request (free vs cross-repo vs paid) — rejects cross-repo operations for free-tier keys with a structured error the Action surfaces as a PR-comment CTA

## Execution playbook

### Phase 0 — Prerequisites (blockers for everything else)

1. **Demo video** (60–90s, landing page). Screen Studio, no face-cam, tight script: problem → dependency graph → install. Don't overthink the first version.
2. **Repo split + license change** (ELv2 on the public repo).
3. **GitHub OAuth + API key issuance + dashboard** — the signup → key → Action snippet flow described in §product surface. Must feel instant (≤60s to first scan config).
4. **Action-first scanner gating** — delete the unkeyed code path; all runs require `CARRICK_API_KEY`; tier enforced server-side (free = single-repo; cross-repo = gated).
5. **Service map pages** with visibility mirroring GitHub repo visibility, linked from the PR comment.
6. **Hardened agent-proxy** — per-key rate limiting, global circuit breaker, repo-size / file-count caps, per-call token cap.
7. **Landing page updates**: replace "Get Early Access" with **"Sign in with GitHub"**. Demo video prominent. No `npx` language — the Action is the install path.

### Phase 1 — The launch spike

**Show HN**. One shot, don't waste it. Fire only when Phase 0 is complete.

- Title: something like *"Show HN: Carrick — a static analyzer for API contract drift across microservices"*. Concrete, not philosophical.
- Positioning: lead with "the missing context layer for AI coding agents" — that's the 2026 wedge, not the CI framing.
- Post Tue–Thu, 8–10am ET. Embed the demo video. Be online answering comments for 12 hours.
- Follow up with Lobste.rs a day or two later for a second wave.

### Phase 2 — Compounding content

These run in parallel after launch. Each post should be publishable on dev.to / Hashnode as well for SEO.

1. **OSS teardown series** (highest leverage). Scan real popular projects with Carrick, publish findings. Requires the tool to be runnable (Phase 0). Report findings responsibly to the project first, then write them up. Candidate pool — vet each against current framework support (express/koa/fastify/hono/nestjs/hapi):
   - Cal.com, Medusa.js, PayloadCMS, Directus, Strapi (Koa), Ghost, n8n, Twenty (NestJS), Rocket.Chat (if applicable)
   - Pick the 3 with the juiciest findings. One post per. Don't commit to 10.

2. **Tactical SEO posts** (search-intent-driven):
   - "Catching API contract drift in a TypeScript monorepo"
   - "OpenAPI vs. static analysis for microservice contracts"
   - "Why integration tests miss API drift"
   - One comparison post: "Carrick vs. Pact vs. OpenAPI codegen"

3. **Thought leadership** (credibility, newsletter shares):
   - "CI is dead. Long live CI" — the agent-attestation piece. Publish *last*, as the capstone. It won't drive SEO, but it's shareable and sets up the 3-year vision conversations with VCs.
   - "Agents are shipping code. Here's what breaks first." — bridge piece between tactical and speculative.

### Phase 3 — Sustained channels

1. **Newsletter sponsorships** (paid, fast, proven): Pointer ($$$ but high-quality devs), Bytes, TLDR Web Dev, Console DevTools. One good slot = 3–10k targeted views. Start with one, measure conversion.
2. **LinkedIn cadence** for engineering leaders — underrated for B2B dev tools. Short posts with screenshots and concrete findings.
3. **YouTube** — one longform (8–12 min) "I scanned 10 OSS monorepos" piece. Evergreen SEO. Clip for Twitter video. One good video per month beats a weekly pace you can't sustain.
4. **Partnerships** — explicit positioning as "the map your AI agent reads before writing code." Pitch integrations with Cursor, Windsurf, Continue.dev, Aider. You already have the MCP server; lean into this.
5. **Podcast circuit** — Changelog, Software Engineering Daily, PodRocket. The "future of CI / agent-native tooling" angle works here where it doesn't work for SEO.

## What to skip

- Product Hunt (dev tools underperform there vs. HN)
- Reddit as a primary channel
- Conferences until there's traction to point at
- A content calendar you can't sustain — one good post/month beats four mediocre ones
- Doing all of the above in parallel — pick 4–5 things and actually finish them
- **Homebrew / npm local CLI** — considered and cut. Cross-repo is the value prop; local single-repo scans undersell it, muddy the messaging, and cut off the user-feedback loop Carrick needs at MVP
- **Application forms for cross-repo access** — one-click request, intro call as the filter

## Priority sequence

1. Demo video
2. Repo split + license change (can run in parallel with video)
3. OAuth + API key issuance + dashboard + Action snippet flow
4. Action-first scanner gating + proxy hardening (per-key limits, circuit breaker, tier enforcement)
5. Service map pages with visibility mirroring
6. Landing page update ("Sign in with GitHub", demo video prominent)
7. Show HN launch
8. First OSS teardown post (within a week of launch, while attention is high)
9. Newsletter sponsorship (week 2–3)
10. Second + third OSS teardown posts
11. Tactical SEO posts (ongoing)
12. "CI is dead" capstone post
13. YouTube piece
14. Partnership outreach

## Decisions resolved

- **License**: Elastic License 2.0 (ELv2) on the public `carrick` repo. Framed as "source-available, not open source." Safe default for VC-track source-available dev tools; blocks SaaS-resale cleanly.
- **Install surface**: GitHub Action only. No Homebrew / npm local CLI at MVP. Reason: cross-repo is the value prop; local single-repo undersells it and severs the user-feedback loop.
- **Signup**: GitHub OAuth, API key minted automatically, dashboard shows prefilled Action snippet. No waitlist, no email form, instant self-serve.
- **Cross-repo gating**: one-click request from dashboard, intro call as the filter (not a form). Manual tier flip by an admin after the call.
- **Map visibility**: mirrors the GitHub repo. Private repos → viewer OAuth + GitHub API read-access check on each view (cached briefly). Public repos → public links.
- **Cost ceiling**: $1k/month for the hosted free tier. Per-key limits sized accordingly; global circuit breaker at ~$800 MTD.
- **Framework support**: goal is full framework-agnostic REST. Currently tested against Express only; other frameworks (koa, fastify, hono, nestjs, hapi) are declared in `framework_detector.rs` but coverage is unverified. Tracked as a separate workstream — see `.thoughts/framework-coverage.md`.

## Dependent workstreams

- **Framework coverage audit** — needed before the OSS teardown series can pick real candidates beyond Express projects. Identify where Express assumptions are baked into the scanner vs. where the agent patterns genuinely work across frameworks. Produce a gap list with test repos per framework.
