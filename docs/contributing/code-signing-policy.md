# Code signing policy

Windows preview setup installers are currently unsigned. SignPath Foundation
code signing is being requested; no SignPath signature or approval is claimed.
macOS previews have separate ad-hoc and Developer ID signed/notarized channels.
Signed previews still identify open dependency license qualification. The production Developer ID release channel
requires signing credentials, notarization and reviewed dependency notices.

ROS Studio source and build scripts are maintained in the
[public repository](https://github.com/netizen-robotics/ros-studio). Windows setup
artifacts are built from that source in GitHub Actions. The setup bundles the
suite's applications with upstream ROS and native dependencies; those upstream
libraries must not be represented as ROS Studio's own signed code.

Danny Ng ([dannyngweekiat](https://github.com/dannyngweekiat)) is the release
contact. The requested SignPath setup uses maintainer review and a manual
approval for each signing request. SignPath roles and provider attribution will
be updated here if enrollment is approved and signing is enabled. Signing is
separate from Store certification and installed-product qualification.

See the application's [privacy and network behavior](../reference/privacy.md).
