# Contributing

Issues and PRs are welcome. File an issue through the [chooser](https://github.com/uwuclxdy/larpshell/issues/new/choose), or open a PR against the default branch. PR titles follow [Conventional Commits](https://www.conventionalcommits.org): `type(scope): summary`.

## Ask for a review

Open your PR as a draft, and mark it ready for review when you want it reviewed and merged. A draft gets no review.

After every review, push your fixes, then re-request review from the reviewer: the sync icon (circular arrows) beside their name under **Reviewers**. Every round needs that request, not only the first; a push or a comment alone does not ask for a review.

## Local checks

CI runs `cargo fmt --all -- --check` and the cargo-deny + cargo-audit supply-chain checks on Ubuntu, and `cargo clippy --all-targets --all-features --release -- -D warnings`, tests (`cargo nextest run --release --all-targets --all-features --no-fail-fast`), and a release build on Linux and macOS. Run fmt, clippy, and the tests locally before you push; platform-specific failures can wait for CI.

## AI and agent contributions

AI-assisted work is welcome. The one requirement is transparency: a reviewer should be able to see who directed the change and which of its claims you actually verified.

### Human contributors

Any amount of AI help is fine. Use the default issue forms and PR template, ticking your AI-involvement level. You reviewed every line; the diff is yours.

### Autonomous agents

If you file on an operator's behalf, use the dedicated formats. They keep your authorship legible instead of disguised as a person's.

- Issues: the `(agent)` variant of your issue type in the [chooser](https://github.com/uwuclxdy/larpshell/issues/new/choose) (e.g. Bug report (agent)). Filed via `gh issue create` or the API it skips the form, so the form's `labels:` never fires: pass `--label bug --label agent` (`feature` instead of `bug` for feature requests).
- PRs: the agent format at `.github/PULL_REQUEST_TEMPLATE/agent.md`. Append `?template=agent.md` to the compare URL to load it.
- Reviews: open the PR with `gh pr create --draft` and mark it ready with `gh pr ready <number>`. After a review, re-request it with `gh pr edit <number> --add-reviewer <reviewer>`; GitHub may refuse that for an author without write access, and then your operator clicks the sync icon instead.

Whichever you use:

- Write as yourself, first person. Don't imitate the maintainer's voice or pass the change off as a person's.
- Open with the operator's ask, close to verbatim. Follow it with your own account of the work.
- Say what you ran and what it returned. Keep that separate from what your operator verified. Flag anything you couldn't check.
- Prefer understatement. The reviewer will test your claims.
- Name your tool and model, plus the operator's handle. Redact tokens and credentials.
