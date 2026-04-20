# GitHub OAuth & Identity

How signup, auth, API key issuance, and map access control work — the contract between `carrick-cloud` and GitHub.

## Why OAuth is load-bearing

GitHub OAuth isn't just a signup gate. It's the thing that makes several product features possible at once:

- **Identity for shareable URLs.** Service maps live at `carrick.tools/<owner>/<repo>/map`. Without identity, private-repo maps can't be gated correctly.
- **Verified metadata.** Username, email, org memberships — all verified by GitHub, no form for the user to fill.
- **Pre-wires the paid conversion path.** We already know their orgs, which sharpens outreach when they click "Request cross-repo access."
- **Private-repo access checks.** For a private-repo map page, viewer access is re-verified at read time using their GitHub API token.
- **Latent viral loop.** Every PR scan comment links to a map URL; co-workers clicking through hit a known-identity page rather than a cold signup form.

## Auth type: OAuth App (not GitHub App) at MVP

GitHub offers two options:

- **OAuth App** — user-level token, scoped by what the user approves. Simpler setup, familiar pattern.
- **GitHub App** — installation-level, can be granted to an entire org, supports webhooks, richer metadata.

**Choice: OAuth App for MVP.** The features we need today (user identity, verify repo access, read org memberships) are all covered. Revisit GitHub App later if org-level installation becomes a clear sales ask — it's a substantial switch, not a small delta.

## Scopes — incremental, not upfront

The signup dialog asking for permissions is the single biggest conversion-killer in a GitHub OAuth flow. Request minimum at signup, escalate only when the user tries to do something that needs more.

**At signup:**
- `read:user` — email + username
- `read:org` — org memberships (enables org-scoped features later)
- `public_repo` — read public repos

**Escalate when needed:**
- `repo` (private-repo read) — requested the first time a user clicks on a private-repo map. In-flow upgrade, not upfront. GitHub supports this via re-auth.

**Do not request `repo` at signup.** It triggers the scary "Carrick will be able to read all your private repositories" dialog, which tanks conversion for a user who just wanted to see the demo.

## Signup flow

```
1. User lands at carrick.tools, clicks "Sign in with GitHub"
2. GitHub OAuth consent screen (read:user, read:org, public_repo)
3. Redirect to carrick-cloud with auth code
4. carrick-cloud exchanges code → access token
5. Fetch user profile, upsert user row
6. Mint API key (shown once, hashed + stored)
7. Redirect to dashboard: prefilled .github/workflows/carrick.yml + secret setup steps
```

Session is cookie-based, standard web app pattern. Access token stored server-side (encrypted at rest), refreshed on expiry, used for subsequent GitHub API calls on the user's behalf.

Target: signup-to-first-scan-configured in **≤60 seconds.**

## API keys

- Minted automatically on first sign-in. No manual "generate key" step.
- Format: prefix-identifiable (e.g. `carrick_sk_live_...`). Prefix enables leaked-key detection in git logs / grep.
- Stored hashed server-side. Shown once in full with copy-button-first UX.
- Rotatable from dashboard (revokes old, mints new).
- Tier-scoped server-side — free-tier keys can't upload for cross-repo even if the scanner tries. The tier check lives on the `carrick-cloud` side, not in the key format.

## Map access control — visibility mirroring

Service maps at `carrick.tools/<owner>/<repo>/map`. Two classes of map:

### Public repos
- Map URL is ungated. Anyone with the link can view.
- `repo_visibility = public` captured at upload time from GitHub's repo metadata API.

### Private repos
- Viewer must be signed in via OAuth.
- `carrick-cloud` calls GitHub's collaborator/permission API (`GET /repos/{owner}/{repo}` or `GET /repos/{owner}/{repo}/collaborators/{username}/permission`) using the viewer's stored GitHub token.
- Cache the access decision briefly (**5 min TTL**) to avoid hammering GitHub's API on page navigation.
- If the viewer lacks `repo` scope, trigger incremental OAuth re-auth.
- **If the check fails, return 404, not 403.** Leaking the existence of a private map is itself a leak.

### Visibility capture
- Captured at scan upload time from the Action's context (`GITHUB_REPOSITORY` + a call to GitHub's repo metadata API using `GITHUB_TOKEN`).
- Stored alongside the map. Re-verified on each map read — a repo that flipped public→private between scans still gates correctly.

## Cross-repo request flow

- Dashboard has a single **"Request cross-repo access"** button.
- Click writes a row to `cross_repo_requests` and emails you.
- Admin surface (MVP: hardcoded admin user ID check on a single admin-only route) lets you flip the user's tier.
- No form. The intro call is the filter.

## Data model

Concrete tables in `carrick-cloud`:

- `users(id, github_id, github_login, email, tier, created_at)`
- `github_tokens(user_id, access_token_encrypted, scopes, expires_at)`
- `api_keys(id, user_id, hashed_key, prefix, created_at, revoked_at)`
- `scans(id, api_key_id, repo_owner, repo_name, repo_visibility, scanned_at)`
- `service_maps(id, repo_owner, repo_name, visibility, storage_key, updated_at)` — keyed on `(owner, repo)`; latest scan wins
- `cross_repo_requests(id, user_id, status, requested_at, reviewed_at)`
- `map_access_cache(user_id, repo_owner, repo_name, has_access, checked_at)` — short-TTL private-repo access cache

## GitHub Action → backend contract

The Action runs in the user's CI. It:

1. Reads `CARRICK_API_KEY` from secrets.
2. Runs the scanner → produces structured facts per file.
3. POSTs to `carrick-cloud` per file for LLM analysis (see `.thoughts/public-private-split.md` for the protocol).
4. Receives structured analysis back.
5. Uploads the final service map + scan metadata to `carrick-cloud/upload` (for free-tier keys this is rejected server-side; single-repo maps are fine, cross-repo upload is not).
6. Posts a PR comment via `GITHUB_TOKEN` with findings + a link to `carrick.tools/<owner>/<repo>/map`.

The Action reports `GITHUB_REPOSITORY` and repo visibility (queried using `GITHUB_TOKEN`) in the upload payload. `carrick-cloud` stores these alongside the map and re-reads visibility on every map view.

## Edge cases to handle before launch

- **User revokes OAuth access on GitHub.** Stored access token stops working. Detect 401s on GitHub API calls, clear the cached token, prompt re-auth on next action.
- **User deleted from GitHub.** Orphan their data or hard-delete? MVP: keep data, flag user as deactivated, disable API keys. Hard-delete on explicit request.
- **Repo deleted or renamed.** Old map URL 404s (correct behavior). No automatic cleanup at MVP — garbage-collect stale maps nightly if storage grows.
- **Org SSO requirement.** Some orgs require SSO for third-party OAuth apps. Handle the "SSO challenge" redirect flow — standard OAuth pattern, don't skip.
- **GitHub API rate limits.** 5000 authenticated requests/hour per user. Private-repo access checks hit this. Short cache + only check on map view keeps this comfortable.
- **Leaked API key.** Prefix enables GitHub secret-scanning partnership (future). MVP: manual revocation from dashboard.

## Phase ordering

Each step is independent and shippable on its own:

1. **OAuth flow + user creation + API key mint + dashboard with Action snippet.** Unblocks self-serve. Public-repo maps only (no visibility gate needed).
2. **Action → backend plumbing** (scanning, uploading, PR comment with map URL). Requires phase 1 for the API key.
3. **Map viewer pages — public repo versions first.** Ungated.
4. **Private-repo map support.** Incremental scope request for `repo`, collaborator check on view, short TTL cache.
5. **Cross-repo request flow** — button + admin toggle. Doesn't block launch; can ship post-Show-HN.
6. **Org-level improvements** (if demand materializes) — switch OAuth App → GitHub App. Non-MVP.

Launch-blocking is 1–3. Private-repo support (4) is *nice to have* for Show HN but not strictly required — public-repo maps demo the feature, private-repo customers can land within a week of launch.

## Cross-references

- `.thoughts/growth-playbook.md` — onboarding flow and tier structure.
- `.thoughts/public-private-split.md` — why the lambda/proxy pieces live in `carrick-cloud`.
