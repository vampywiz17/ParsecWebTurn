# Pull request code quality

`Code quality` runs for PR opening, pushes to an open PR, reopening and marking
a PR ready for review. It also runs on `dev` pushes or manual dispatch. Results
appear in the PR's Checks tab; they do not approve or merge the PR.

- **Clippy (Windows):** application, BUD workspace, video adapter, QoS tool and
  TURN integration tests, including all features and test targets. Warnings fail
  the check. This covers the Windows implementation, not an unmerged macOS port.
- **Dependency policy:** `cargo-deny` 0.20.2 checks the five dependency graphs
  against RustSec, the license list and source policy in `/deny.toml`. Git
  dependencies must use a revision. Multiple versions are warnings because
  upstream WebRTC/vendor APIs legitimately require different major versions.
- **Workflow lint:** `actionlint` 1.7.12 checks every GitHub Actions workflow.
  On Linux it also uses the runner's ShellCheck when available.

The two downloaded checker archives are pinned by version and SHA-256. No tool
is linked into or distributed with the client. Jobs use disposable GitHub-hosted
runners, read-only repository permissions and no persisted checkout credentials.
They do not use repository secrets or `pull_request_target`.

GitHub can still require approval for a first-time contributor who is new to
GitHub. That platform requirement is separate from PR review/merge approval.
The repository uses the least restrictive public-fork approval policy offered
by GitHub. Existing waiting runs may still need a maintainer to start them.

Do not suppress a check just to make it green. Investigate Clippy findings before
changing runtime code, especially around GPU ownership, queues and input. Any
necessary advisory/license exception must be narrow and justified in the policy.
These checks do not establish live host or hardware compatibility.
