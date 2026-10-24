# Instructions for agents

- Use `uv` instead of raw python
- ruthlessly apply the KISS principle: keep it simple, avoid unnecessary complexity, and focus on the core functionality.
- use gitmoji when committing
- Run all operations involving the Rust toolchain (including builds, tests, formatting, and Clippy) inside the project's `.devcontainer`. If not already running inside the devcontainer, execute those commands in it rather than using or installing a local Rust toolchain.
- Always run `cargo build --release` inside the project's `.devcontainer` after completing a change, before reporting it as done.

## Testing

- Test observable outcomes at the narrowest stable boundary that expresses the requirement. A pure function's return value and a Git adapter's generated command are outcomes at their respective boundaries.
- Application and workflow tests use `FakeGit` (`tests/common/mod.rs`) with realistic status, history, file, and diff responses, then assert the resulting rendered state. Do not invoke real repositories.
- Adapter tests in `src/git.rs` use the recording `Runner` and may assert argv, working directory, environment, allowed exit codes, parsed results, and error text. These protect path safety, repository scope, porcelain usage, and command semantics without testing Git itself.
- Prefer the rendered `TestBackend` buffer (visible text, and cell styles when styling is the behavior), focus, enabled or dimmed buttons, scrolling, copied text, toasts, and `FakeGit` calls over private fields, request tokens, object identity, or helper call order.
- Never assert literal colors. Render the component in the relevant state and compare cells, for example "keyword cells differ from plain cells".
- Test caching, virtualization, and threading only when they support an explicit behavioral or concurrency requirement. Assert the requirement, not incidental implementation mechanics. The one performance contract is that rows materialized per draw stay bounded by the viewport height.
- Unit and component tests live in `#[cfg(test)]` modules next to the code. Workflow tests go in `tests/*.rs`, organized by product behavior; keep complete user workflows together and files focused. Put shared support in `tests/common/mod.rs` only when at least two test files need it.
- Keep the `src/code_view.rs` tests as the `CodeView` component boundary. Do not fold widget internals into workflow tests.
- Quality gates: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.

## Project context

- Read `docs/milestones.md`, `docs/design-spec.md`, or `docs/workflow.md` only when the task concerns their milestone, design, or workflow.
