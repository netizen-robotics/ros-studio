# Save and restore a workspace

Save a workspace to reuse display or inspection configuration. Workspaces store intent, not a recording of your ROS session.

## Save your configuration

Use the application's **File** menu to save your workspace. Visualizer and Tools use separate formats. A workspace from the wrong application is rejected.

Workspace files can contain source names, configuration, and asset paths. Review those values before sharing a file, especially when it refers to private systems or local files.

## Restore without restarting operations

Opening a workspace previews compatibility before replacement. Supported older formats import through a preview that preserves the original file and identifies exclusions.

Restored work stays inactive. Reconnect and start reception explicitly. Robot assets remain unloaded until you load them, and Tools does not replay commands.

## Recover from an error

Read the error details before replacing the current workspace. Keep the original file when investigating a compatibility or save failure. Use the application's recovery controls if cleanup must finish before another file operation.

A native file chooser can interfere with macOS Cmd-Q. Close or cancel the chooser before quitting; see [known limitations](../reference/limitations.md).

Older Visualizer workspaces may retain several saved modes for one topic. Their settings remain recoverable through the mode selector, while activation permits one mode per topic.
