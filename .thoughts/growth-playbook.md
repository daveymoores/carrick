# Carrick Growth Playbook

The goal: drive real signups and usage traction before the VC demo window closes. This is the consolidated plan.

## Honest baseline

- Prior blog post: 3k views → 2 signups (0.07%). The old site was the bottleneck; new site should do 1–3% on the same audience.
- Current blocker is **friction**, not just traffic. Waitlist-only + no demo video + no way to try = dead funnel on an HN launch.
- Demo video is the single highest-leverage unlock.
- The "closed source protects the moat" frame is misleading: the scanner is the hard engineering, but standalone it's incomplete. The commercial moat is the hosted backend (cross-repo data gravity, billing), not the scanner code.

## The core unlock: repo split + hosted free tier

This is the change that makes everything else work. Without it you can't do the OSS teardown posts, can't launch on HN without a "another waitlist" backlash, and can't convert HN traffic at scale.

### Repo split

**New public repo** — `carrick` (BSL 1.1 or Elastic License 2.0, source-available):
- `src/` — Rust scanner, framework detector, agents, call-site classifier
- `ts_check/` — TypeScript type-checking sidecar
- `mcp-server/` — client-facing MCP piece
- `action.yml` — GitHub Action entry point
- Scanner runs against a single repo locally with zero config. No key needed.

**New private repo** — `carrick-infra`:
- `lambdas/` — agent-proxy, check-or-upload, mcp-server lambda
- `terraform/` — all AWS infra
- Billing, org management, API-key issuance

BSL/ELv2 explicitly prevents anyone hosting it as a competing SaaS. Sentry, HashiCorp, CockroachDB, MariaDB use this model.

### Hosted free tier (replaces the waitlist)

The agent-proxy is load-bearing — LLM calls are core to the tool. BYO keys introduces model inconsistency you don't want to support. So host it, with tiered access:

| Tier | Who | Limits | Access |
|---|---|---|---|
| **Anonymous** | HN click, OSS teardown readers | 1 scan/day per IP, ≤500 files, global circuit breaker | Single-repo only |
| **Free key** (GitHub OAuth signup) | Prospects who've seen it work | 5 scans/day for 30 days | Single-repo only |
| **Paid** | Real customers | Unlimited | Cross-repo + MCP + dashboard |

**Cost ceiling: $1k/month.** At ~$0.20/scan average that's ~5,000 scans. Size the limits so you can't blow past it:
- Anonymous: 1/day/IP, global daily cap ~100 scans (~$20/day, ~$600/month headroom)
- Free-key: ~$400/month headroom for the GitHub-authenticated tier
- Hard circuit breaker at ~$800 month-to-date spend with graceful degradation message

**GitHub OAuth over email for the free-key tier.** Product already lives in GitHub (the Action, the repo data), so it's consistent. Zero added friction for devs. Gives you verified email + username + org memberships — far better signal than an email form, and it pre-wires the paid-conversion path (you already know their org).

### Scanner-side gating

- No `CARRICK_API_KEY` present → `carrick scan .` runs locally, calls hosted proxy anonymously (rate-limited by IP), skips `cloud_storage/` upload and cross-repo fetch.
- `CARRICK_API_KEY` present → unlocks upload, cross-repo analysis, MCP queries.
- Print a clear CTA when cross-repo features are skipped: "cross-repo requires a key — get one at carrick.tools".

### Infra work needed on the proxy

The current `agent-proxy/index.js` has only a naive global 2000/day counter. For a public free tier you need:
- Per-IP rate limiting (DynamoDB or API Gateway usage plans)
- Global circuit breaker with graceful "come back tomorrow" message
- Repo-size/file-count cap enforced before accepting the scan
- Per-call token cap so one pathological file can't burn $5

## Execution playbook

### Phase 0 — Prerequisites (blockers for everything else)

1. **Demo video** (60–90s, landing page). Screen Studio, no face-cam, tight script: problem → dependency graph → install. Don't overthink the first version.
2. **Repo split + license change** (BSL or ELv2 on the public repo).
3. **Free-tier gating** in the scanner (anonymous + free-key paths).
4. **Hardened agent-proxy** with per-IP limits + circuit breaker.
5. **Landing page updates**: replace "Get Early Access" with "Try it now" → `npx carrick scan .`. Keep email capture for the free-key tier.

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

## Priority sequence

1. Demo video
2. Repo split + free-tier gating + proxy hardening (can run in parallel with video)
3. Landing page update
4. Show HN launch
5. First OSS teardown post (within a week of launch, while attention is high)
6. Newsletter sponsorship (week 2–3)
7. Second + third OSS teardown posts
8. Tactical SEO posts (ongoing)
9. "CI is dead" capstone post
10. YouTube piece
11. Partnership outreach

## Decisions resolved

- **Cost ceiling**: $1k/month for the hosted free tier. Limits sized accordingly (see table above).
- **Signup auth**: GitHub OAuth for the free-key tier. Consistent with product surface; better signal than email.
- **Framework support**: goal is full framework-agnostic REST. Currently tested against Express only; other frameworks (koa, fastify, hono, nestjs, hapi) are declared in `framework_detector.rs` but coverage is unverified. Tracked as a separate workstream — see `.thoughts/framework-coverage.md` (to be written).

## Dependent workstreams

- **Framework coverage audit** — needed before the OSS teardown series can pick real candidates beyond Express projects. Identify where Express assumptions are baked into the scanner vs. where the agent patterns genuinely work across frameworks. Produce a gap list with test repos per framework.
