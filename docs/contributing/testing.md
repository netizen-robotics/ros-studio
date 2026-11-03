# Test a change

Choose checks that exercise the behavior you changed. Keep automated, native UI, real ROS, packaging, and hardware results separate.

## Run focused offline checks

Use locked Cargo commands through `scripts/dev`. For example, a network-crate change can use:

```sh
scripts/dev cargo test --locked -p studio-network
scripts/dev cargo clippy --locked -p studio-network --all-targets -- -D warnings
scripts/dev cargo fmt --all -- --check
```

Build the affected applications or libraries too. Add regression coverage for meaningful failure, cancellation, stale-event, and backpressure behavior. Ignored opt-in tests are not passes.

For native UI changes, inspect the compiled application with synthetic data. Verify state after each action, keyboard focus, resizing, long values, errors, and cleanup. Record the executable hash and keep screenshots private until reviewed.

## Test with synthetic ROS data

Use two terminals from the repository root. Restrict discovery to localhost and domain `187`, and select the same middleware in the publisher and application.

Set these values in both terminals:

```sh
export ROS_DOMAIN_ID=187
export ROS_LOCALHOST_ONLY=1
export ROS_AUTOMATIC_DISCOVERY_RANGE=LOCALHOST
export SKIP_DEFAULT_XML=1
export RMW_IMPLEMENTATION=rmw_fastrtps_cpp
export STUDIO_UI_ROS_EVIDENCE="$PWD/artifacts/feasibility/evidence/local-test"
```

Use a new evidence directory for each run. Remove remote peers and custom discovery profiles first; the fixture guards reject conflicting settings.

In the publisher terminal, inspect the fixture instructions, then start it:

```sh
scripts/dev .pixi/envs/default/bin/python scripts/ui-ros-workflow.py
scripts/dev .pixi/envs/default/bin/python scripts/ui-ros-workflow.py --publisher
```

In the application terminal, launch Tools through the fixture's backend recorder:

```sh
STUDIO_BACKEND="$PWD/scripts/ui-ros-backend" \
  scripts/dev cargo run --locked -p studio-tools
```

Choose **Local computer**, domain **187**, and **Fast DDS** in Tools. Connect, then explicitly receive `/studio_fixture/ui/reliable`. Test QoS mismatch and correction with `/studio_fixture/ui/best_effort`, and late reception with `/studio_fixture/ui/retained`.

Verify selection without replacement, explicit source replacement, Stop, and reconnect without replay. Stop the owned publisher with Ctrl-C. Repeat separately with `rmw_cyclonedds_cpp` and Cyclone DDS selected in the UI. Serialize native and ROS runs so processes and evidence remain attributable.

For the Map replacement/TF-delay regression, build `studio-backend`, then run `scripts/dev .pixi/envs/default/bin/python scripts/test-map-tf-lag-ros.py --label map-tf-lag-local` with a fresh label for each run. The helper serializes matching Fast DDS and Cyclone DDS cases on localhost/domain `187`, testing short-delay retention, exact acquisition-time transforms, the bounded timeout under continuous input, and explicit Stop. It writes local evidence and stops only its owned test processes.

For the supported display-family continuity sweep, run `scripts/dev .pixi/envs/default/bin/python scripts/test-topic-continuity-ros.py --label topic-continuity-local` after building `studio-backend`. It serializes matching Fast DDS and Cyclone DDS on localhost/domain `187`: acquisition-time displays exercise a short TF gap, exact-stamp recovery, bounded invalidation during continuous unresolved input, and Stop. Raw Image, Messages, Plot, TF, and Robot exercise initial/replacement reception and Stop. Use `--kinds` for a focused retest and a fresh label to preserve previous results. These backend checks do not establish native rendering, arbitrary message types, maximum-load combinations, LAN, or physical robot acceptance.

For image-size acceptance, use `--kinds image camera --image-size 1920 1080 --reception-only` with that helper and a fresh label. It checks exact RGB bytes and dimensions, replacement reception, and Stop on both matching middleware pairs. This mode uses an identity Camera frame to isolate payload-size handling; it does not qualify high-resolution acquisition-TF timing or sustained frame rates. Rust tests cover the 2048×2048 RGBA boundary, padding, oversize rejection, queue pressure and frozen session20 compatibility.

Use `--kinds image --image-size 640 480 --image-encoding 16UC1 --reception-only` (or `32FC1`) for exact depth payload reception. The helper generates explicit loopback-only transport profiles for both vendors. Image component tests check metric conversion, endian order, padding and invalid depth values; `/demo/camera/depth16` and `/demo/camera/depth32` provide VGA gradients for native inspection without ROS traffic.

For spatial fixtures, read the [robot](../../fixtures/robot_visualizer/README.md), [sensor](../../fixtures/sensor_scene/README.md), and [mapping](../../fixtures/mapping_scene/README.md) guides.

## Record and review evidence

Full records need OS/CPU, ROS distribution, dependency versions, middleware pair, exact command, outcome, and evidence paths. Distinguish passed, failed, and not tested. Keep failed runs and pre/post-fix binary identities.

Generate and check the local evidence index after tests:

```sh
scripts/dev .pixi/envs/default/bin/python scripts/snapshot-evidence.py
scripts/dev .pixi/envs/default/bin/python scripts/snapshot-evidence.py --check
```

The index lives under ignored `artifacts/feasibility/RESULTS.json`. It summarizes available local reports; it does not rerun tests or replace raw evidence. A fresh checkout has no local evidence until tests produce it.

CI uploads generated evidence on success and failure. Review any evidence you publish or export for personal paths, hostnames, addresses, interface names, message contents, and credentials. Private product planning does not belong in test artifacts.
