# Repository instructions

## Public documentation and product scope

ROS Studio is one installed product containing independent Visualizer, Tools, and Launcher processes. Read [public documentation](docs/README.md), [architecture](docs/contributing/architecture.md), and [known limitations](docs/reference/limitations.md) before changing behavior. Visualizer owns spatial displays, calibrated Camera, and Image; Tools owns Topics/Services/Actions/Nodes/Parameters and Messages/Plot/Image. Launcher opens bundled tools. Windows delivery uses one Store MSIX; macOS uses one signed/notarized app in a DMG. Update the suite as a unit; do not restore per-tool downloads or an elevated helper in Store builds. Retain `studio-desktop` until replacement acceptance is complete.

Public docs describe current behavior, user tasks, and contributor procedures. Keep product plans, design work orders, and historical acceptance ledgers outside this repository. A maintainer can supply private planning context for an assigned slice; a fresh checkout must not depend on that folder. Do not invent feature scope, release promises, or closure evidence when private context is unavailable.

Do not expand into navigation commands, physical robot operation, additional rqt-style tools, a robot companion, or a RustDDS backend replacement without explicit agreement. Rust-first permits standard ROS C/C++ libraries through the narrow backend boundary.

## Architecture and dependencies

Separate native UI, versioned local protocol, and distro-specific backend processes. Never mix ROS distributions in one process. Keep network/ROS work off the UI thread. Bound queues and report dropped samples. Commands never replay on reconnect. Observe mode is the default; interaction requires an explicit session permission and operation.

Use project-local Pixi environments and caches, and commit dependency lockfiles. Do not modify shell profiles or system Python. Runtime users must not need ROS setup scripts or developer tools. Native library licensing and relocation are part of packaging.

## Safety and ownership

Use synthetic fixtures. Never command a physical robot without explicit authorization. Preserve unrelated changes, raw evidence, environments, and Git history. Do not commit, amend history, push, publish, or deploy unless requested. No sub-agent delegation unless explicitly requested. Use one integration owner and serialize native/ROS/simulator acceptance and evidence indexing.

## Checkpoints

Each checkpoint has one user-visible outcome and at most five concrete steps. Define them before starting with `scripts/dev .pixi/envs/default/bin/python scripts/checkpoint.py`. Check status before another validation or repair cycle. State lives under ignored `artifacts/feasibility/checkpoints/`.

Do not hide extra work inside a step, rename an incomplete outcome to reset its allowance, or repeatedly rerun unchanged broad checks. Repair/retest cycles have no fixed numerical limit; record diagnosis, change, and focused retest. At the step cap, close as passed or unresolved and report exact remaining failures. Explicit authorization is required to reopen an unresolved checkpoint.

## Development cycle

1. Ground the assigned scope in source, public docs, relevant issues, and any supplied private plan. Check Git status and preserve existing work. Define acceptance and limits before implementation.
2. For UI changes, define the primary task, configuration flow, and empty/starting/waiting/receiving/stopped/error/recovery states. Keep advanced configuration progressive and unavailable capabilities out of active UI.
3. Implement a bounded slice. Keep drafts separate from active configuration. Version intentional protocol changes and test compatibility rather than silently changing a frozen contract.
4. Run focused tests, builds, formatting, and Clippy through locked Pixi tooling for affected Rust code. Exercise failure, cancellation, stale-event, and backpressure paths. Documentation-only work needs link/reference/privacy checks, not native/ROS reruns.
5. For UI changes, inspect the actual compiled window with synthetic fixtures. Observe resulting state, keyboard focus, resizing, long values, and error recovery. Save screenshots and executable hashes. Headless checks do not establish native or accessibility acceptance.
6. For ROS-facing changes, test the actual backend with local synthetic publishers after protocol checks. Restrict peers to localhost/domain187, select middleware explicitly, and remove remote/custom discovery settings. Test matching Fast/Fast and Cyclone/Cyclone where available. Cover explicit subscribe, selection without replacement, QoS mismatch/correction, retained data, replacement, Stop, and reconnect without replay as applicable.
7. Verify affected startup cancellation, backend failure, disconnect, window close, and macOS Cmd-Q. Use the native exit callback to request cancellation and wait for worker cleanup; `run_native` return and App drop alone do not reliably cover Cmd-Q in the current eframe version. Keep kill/wait off active UI frames. Confirm owned backend/publisher processes are gone; stop only processes identified as part of the current test.
8. Record results and limitations, update public docs for behavior changes, and update private plans only when supplied for the task. Preserve failures before repairs and keep pre/post hashes separate. Generate the evidence index, check it, and report delivered behavior plus passed/failed/not-tested results and unresolved items.

## Evidence policy

Keep generated JSON reports, logs, message samples, inventories, captures, and screenshots under ignored `artifacts/`. Never place generated run records in public docs or force-add ignored evidence to Git. Preserve failed and superseded runs with descriptive labels rather than overwriting them.

Full results retain OS/CPU, ROS distribution, dependency versions, middleware pair, exact command, outcome, and evidence paths. A build does not prove native execution, clean-machine packaging, LAN behavior, or hardware acceptance. Containers do not prove macOS/Windows compatibility. Do not mark unavailable targets passed or aggregate overlapping runs into a release score.

Run `scripts/dev .pixi/envs/default/bin/python scripts/snapshot-evidence.py`, then the same command with `--check` after tests. The generated index stays in ignored `artifacts/feasibility/RESULTS.json`; it checks report consistency and does not rerun feature tests. A fresh checkout without local reports has no evidence to validate.

Share detailed evidence only through reviewed CI uploads or explicitly requested exports. State when evidence is local-only. Review artifacts for private identifiers and payloads before sharing. CI must retain relevant generated evidence on failure as well as success; private planning is never a CI input or artifact.

## Validation helpers

- `scripts/ui-workspace-quality.py`: retained desktop build/tests/Clippy/format and evidence indexing; use focused equivalents for other subsystems
- `scripts/ui-workspace-demo` and `scripts/ui-workspace-fixture.py`: development-only protocol fixtures without ROS/network traffic
- `scripts/ui-ros-workflow.py`, `scripts/ui-ros-backend`, and `scripts/test-ui-ros-workflow.py`: localhost publishers, backend recorder, and isolation guards
- `scripts/test-snapshot-evidence.py`: offline checks for fresh-checkout indexing, report changes, and historical-record preservation

Run Python helpers with the project Python through `scripts/dev`. Follow the [testing guide](docs/contributing/testing.md) for two-terminal ROS fixtures and evidence handling.
