# Contribute to ROS Studio

Start with a focused bug report, documentation correction, or implementation change. Explain the problem and the expected behavior so reviewers can assess the result.

## Prepare your change

Read [build from source](docs/contributing/building.md) and the [architecture guide](docs/contributing/architecture.md). Keep public documentation about current behavior and contributor tasks. Product planning, implementation work orders, and private operator records are maintained separately from the public repository.

For a feature proposal, describe the task you want to complete, its scope, and its effect on existing workflows. A proposal does not change the supported feature set or promise a release date.

## Keep changes focused

Preserve unrelated work. Keep ROS and network operations off the UI thread, bound queues, and report dropped samples. Reconnect must not replay commands. Use synthetic publishers for tests; physical robot operations require explicit operator authorization.

Use project-local Pixi environments and locked dependencies. Do not add requirements to modify shell profiles or system Python. Follow [repository instructions](AGENTS.md) when using coding agents.

## Validate and submit

Run the [checks relevant to your change](docs/contributing/testing.md). Documentation-only changes need link, command-reference, and privacy checks; they do not need a new native or ROS run.

Describe what changed, why, and how you tested it. Distinguish passed, failed, and not-tested cases. A build does not prove native UI behavior, packaging, LAN compatibility, or hardware acceptance.

Keep generated evidence under ignored `artifacts/`. Review logs, screenshots, workspace files, and diagnostics before sharing them. Do not commit credentials, local addresses, personal paths, or private planning documents.

## License contributions consistently

Submit original contributions under the project's [MIT license](LICENSE). Preserve existing third-party notices and identify any separately licensed material in your change. See [third-party notices](THIRD_PARTY_NOTICES.md) for bundled assets and adapted code.
