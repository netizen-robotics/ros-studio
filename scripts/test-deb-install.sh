#!/bin/bash
# Run only inside a disposable Ubuntu 24.04 container with /packages mounted read-only.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y /packages/ros-studio_*_amd64.deb
root=/opt/ros-studio
for role in launcher tools visualizer backend; do
  test -x "$root/studio-$role"
  ldd "$root/studio-$role"
done > /tmp/ros-studio-linkage.log
find "$root/runtime/lib" -type f -name '*.so*' -exec ldd '{}' \; >> /tmp/ros-studio-linkage.log
if grep -E 'not found|/\.pixi/|/home/runner/' /tmp/ros-studio-linkage.log; then
  echo 'Missing library or dependency on build host'; exit 1
fi
for command in ros-studio ros-studio-tools ros-studio-visualizer; do
  env -i PATH=/usr/bin:/bin HOME=/tmp "$command" --suite-configuration
 done
# Invalid argument validates backend linkage without creating a ROS context.
set +e
env -i PATH=/usr/bin:/bin HOME=/tmp "$root/studio-backend" --package-linkage-check > /tmp/backend-check.log
result=$?
set -e
test "$result" -eq 1
grep -q 'Usage: studio-backend' /tmp/backend-check.log
test -f "$root/runtime/share/ament_index/resource_index/packages/rcl"
test -f /usr/share/applications/ros-studio.desktop
test -f /usr/share/icons/hicolor/150x150/apps/ros-studio.png
dpkg -V ros-studio
mkdir -p /root/.local/share/ros-studio
printf 'preserve user data\n' > /root/.local/share/ros-studio/package-smoke-marker
apt-get install --reinstall -y /packages/ros-studio_*_amd64.deb
test -f /root/.local/share/ros-studio/package-smoke-marker
apt-get purge -y ros-studio
test ! -e "$root"
test ! -e /usr/bin/ros-studio
test ! -e /usr/share/applications/ros-studio.desktop
test -f /root/.local/share/ros-studio/package-smoke-marker
printf 'PASS: clean-container install, ELF linkage, frontend configuration, backend argument validation, reinstall and purge. No GUI or ROS network acceptance.\n'
