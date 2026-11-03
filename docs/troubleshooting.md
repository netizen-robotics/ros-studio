# Troubleshoot connection and reception problems

Check discovery, source settings, and display status separately. Start with a local synthetic publisher when you need to distinguish an application problem from a network problem.

## No topics appear

Confirm that the backend was built beside the frontend, then check these settings:

1. Match the publisher's ROS domain.
2. Select the intended network mode and physical adapter.
3. Match Fast DDS or Cyclone DDS on both sides.
4. Check for inherited localhost, static-peer, or middleware-profile settings.
5. Review local-network permissions and backend-scoped firewall rules.

Disconnect before changing connection settings. A successful ping does not establish DDS discovery. See [configure networking](guides/networking.md) for adapter and custom-profile behavior.

## A topic appears but no samples arrive

Confirm the exact topic type and explicitly enable reception. Match the publisher's quality of service (QoS). A reliable subscriber cannot receive from a best-effort-only publisher.

For a once-published sample, check whether the publisher retains it and whether the subscriber uses compatible transient-local durability. Some retained-data and maximum-map cases remain unresolved; consult [known limitations](reference/limitations.md).

## A spatial display stays empty

Check the display's error and waiting states. Verify the fixed frame and the transform chain at the sample's acquisition time. System time is the current default; simulation-clock handling is incomplete.

Try raw Image inspection when you need to separate image reception from camera calibration or scene-transform problems. Raw Image does not need a scene transform.

## The backend does not start

Build the frontend and `studio-backend` from the same checkout. By default, the application looks for its backend beside its executable. An explicit `STUDIO_BACKEND` override can point to an incompatible or missing build.

For macOS development launches, keep `DYLD_LIBRARY_PATH`, `DYLD_FALLBACK_LIBRARY_PATH`, and `DYLD_INSERT_LIBRARIES` unset. Apply ROS library configuration to the backend rather than the native frontend.

## Collect diagnostics

Use the **Diagnostics** status-bar action to review a frozen report while the problem is present, then use its copy action. **Cmd/Ctrl+Shift+D** also copies diagnostics. Diagnostics collection does not start a ROS operation and uploads nothing automatically.

Reports exclude message payloads, request drafts, parameter values, environment variables, and local paths. They can include interface names and application state, so review the report before posting it publicly. If the entire event loop is blocked, the shortcut may not respond; describe that separately.

When reporting a bug, include the app, OS/CPU, ROS distribution, middleware, reproduction steps, expected result, and observed result. Remove private identifiers from screenshots and logs.
