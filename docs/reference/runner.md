# Running a scan from a runner

The scanner's side of the contract with a process that runs `carrick` on a
checkout it made itself: a container the cloud starts, or any CI other than
GitHub Actions. A GitHub Actions run needs none of this; the Action passes no
flags and the scan reads its context from `GITHUB_*`.

## The command

```
carrick <path> --repo <owner/name> --upload
```

- `--repo <owner/name>` is the repository the checkout is. It decides:
  - the name the scan compares its stored blobs against: which stored
    generation it reuses, so whether unchanged files replay their answers or
    go to the model again;
  - which downloaded copies are this repository's own and which are siblings
    in the cross-repo join;
  - the repository `start-scan` and the preflight-failed marker name.

  Without it the repository is the basename of `GITHUB_REPOSITORY`, else the
  checkout folder's name, and the full name sent to the cloud comes from the
  origin remote. When the flag and the origin remote disagree, the run logs it
  at info and uses the flag. The value must be `owner/name`; anything else is a
  usage error.
- `--upload` / `--no-upload` decides whether the scan uploads its index.
  Without either, the scan decides from the GitHub context: no upload on a
  `pull_request` event or a `refs/pull/*` ref, upload on `refs/heads/main` and
  `refs/heads/master` only, and upload when nothing is set.
  `CARRICK_OUTPUT_JSON` never uploads, and `--upload` with it is a usage error.
  A `--no-upload` run that opened a scan and finished ends it with
  `close-scan`, reason `no_upload`, so its slot is released straight away. A
  run that failed reports `scan-failed` instead.

Neither flag is listed in `carrick --help`.

## Environment

| Variable | What it does |
|---|---|
| `CARRICK_TOKEN` | The credential. Overrides the file `carrick login` writes. |
| `CARRICK_RUN_ID` | The run this scan belongs to, so the scan's log and the runner's join on one key. |
| `CARRICK_RUN_REPO_COUNT` | How many repositories the run covers, so the cloud waits for all of them before it mails. |
| `CARRICK_SIDECAR_MAX_OLD_SPACE_MB` | The type checker's V8 heap cap, in MB. `0` passes no cap. |
| `CARRICK_ALLOW_UNPREPARED` | Scan a checkout whose dependencies are not installed instead of refusing it. |

## Clone shape

The folder the checkout lives in can have any name once `--repo` is passed.
For unchanged files to replay their stored answers, the commit each service's
stored blob was written at must be present in the clone; carrick#2212 covers
partial and shallow clones.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | The scan finished. What it still owes (pending services, losses allowed by `CARRICK_ALLOW_PARTIAL_ANALYSIS`, budget refusals) is in its summary. |
| 1 | The scan failed, and the stage was reported to the cloud (`scan-failed` or `preflight-failed`). Includes an unknown argument to the bare scan. |
| 2 | Usage: an unknown command, a local command's parse error, a `--repo` value that is not `owner/name`, or `--upload` with `CARRICK_OUTPUT_JSON`. |
| 3 | Reserved for carrick#2208: the type checker ran out of memory (opt-in). |
| 101 | Panic. It is first reported as `scan-failed` with reason `panic` (`src/panic_report.rs`). |
| 129 / 130 / 143 | SIGHUP / SIGINT / SIGTERM (`src/shutdown.rs`). 143 is also an ECS `StopTask` or a Spot reclaim. |
| other | Killed from outside. 137 is SIGKILL, which on ECS is usually the task's memory limit: read the stop reason. |
