# Privacy and network communication

The bundled ROS Studio applications do not include analytics or automatic crash
reporting. Workspaces, preferences and application diagnostics are stored on the
computer where the applications run. ROS messages can contain private data;
review a workspace or diagnostic file before sharing it.

Connecting an application starts a local ROS backend. That backend discovers and
communicates with ROS participants according to the domain, middleware and
network profile you select. Receiving data and using explicit Tools operations
can exchange data with those participants. Review the selected network and
target system before connecting or performing an operation.

Launcher opens the installed tools. The bundled suite does not automatically
download applications or send usage reports to Netizen Robotics. Clicking its
website link opens the Netizen Robotics website in your browser; that website
has its own privacy practices. Downloading a release from GitHub is also subject
to GitHub's privacy policy.

Uninstalling the Windows setup removes application files and retains user
workspaces and preferences under `%LOCALAPPDATA%/ROS Studio`. Remove those files
yourself if you also want to erase the saved application data.
