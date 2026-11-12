# Build release candidates on GitHub

Use **Actions → Release candidate → Run workflow** on the reviewed source revision.
This workflow produces downloadable Actions artifacts; it does not publish a
GitHub Release or submit an application to either store.

For build-only checks, use **Suite CI (Windows, macOS, Ubuntu)**. Its `platform`
selector defaults to `all`; select one platform for a focused repair/retest.
The suite workflow and release-candidate workflow are manual, not push/PR checks.
Ubuntu is also available independently through the `ubuntu-check` channel below.

## Channels

| Channel | Output | Required environment |
| --- | --- | --- |
| `macos-preview` | Ad-hoc ARM64 DMG for development evaluation | None |
| `macos-dmg` | Developer ID signed and notarized ARM64 DMG | `macos-dmg` |
| `windows-setup` | Unsigned per-user x64 setup EXE with the full offline suite | None |
| `microsoft-store` | Unsigned x64 MSIX for Partner Center submission | `microsoft-store` |
| `ubuntu-check` | Ubuntu 24.04 x64 build, unit-test, formatting and Clippy evidence | None |
| `ubuntu-deb` | Experimental Ubuntu 24.04 amd64 DEB and install-test evidence | None |

The Ubuntu channels use the locked Jazzy Pixi environment on a native Linux
runner. It does not require Apple or Microsoft credentials. Logs and dependency
records are retained even when checks fail. `ubuntu-check` produces evidence only.
`ubuntu-deb` bundles the Launcher, Tools, Visualizer and private Jazzy runtime
under `/opt/ros-studio`. It checks ELF dependencies and install, configuration,
reinstall and purge in a clean Ubuntu container before uploading the DEB.
These checks do not qualify graphical UI, LAN behavior or Ubuntu ARM64 support.

## Download the first preview packages

Run `macos-preview`, `windows-setup`, and `ubuntu-deb` against the same reviewed
source commit. Download the corresponding artifacts from each run's Summary.
These channels need no signing credentials and do not submit to either Store or
publish a GitHub Release. The macOS preview is ad-hoc signed; the Windows setup
is unsigned. Production `macos-dmg` retains its signing and reviewed-notice gates.

The Windows setup uses Inno Setup 6 on `windows-2025`. It installs Launcher,
Visualizer, Tools and one private Jazzy runtime into
`%LOCALAPPDATA%/Programs/ROS Studio`, creates a Start menu entry and registers an
uninstaller. It requests no administrator access and includes no firewall helper.
Close every suite window before installing an update. Windows Settings → Apps →
Installed apps removes the suite; workspaces and preferences are retained.
The unsigned installer may display publisher/reputation warnings. Store
certification and SignPath approval are separate from generating this installer.

The Windows job verifies installation, payload hashes, executable linkage with
build-environment paths removed, reinstall and uninstall on its disposable CI
runner. This runner has developer tools installed: these checks do not establish
clean-machine, graphical UI, Windows 11 client or LAN acceptance. Dependency
notice review remains open across the preview packages. The setup artifact
contains the EXE, SHA256SUMS, payload hashes and package metadata.

## Download and install the Ubuntu preview

Run the `ubuntu-deb` channel. After a successful run, download and extract the
`ros-studio-ubuntu-deb-<commit>` artifact from the run's Summary page. It contains
the DEB and `suite-package.json` with its SHA256. Artifacts are retained for 14 days.
On Ubuntu 24.04 amd64, in the extracted directory:

```sh
sudo apt install ./ros-studio_*_amd64.deb
ros-studio
```

The desktop application menu also contains **ROS Studio**. The commands
`ros-studio-tools` and `ros-studio-visualizer` open individual suite processes.
Users do not need Pixi, ROS setup scripts or compiler tools. Remove the package
with `sudo apt remove ros-studio`; user data is preserved. This is an unsigned
experimental download, without an APT repository or automatic package updates.
Complete dependency license review and native desktop/ROS acceptance remain
required before a public production release.

Preview packages do not establish public distribution readiness. The Microsoft
Store signs the MSIX after certification; the unsigned artifact is not a direct
public installer. First publication is completed through Partner Center.

Mac App Store packaging is not yet implemented. Its application certificate
cannot replace Developer ID signing. Sandboxed backend execution, discovery,
file access, nested app signing, provisioning and installed-product acceptance
must be validated before a Mac App Store channel is enabled.

## Configure environments

Under repository **Settings → Environments**, create the release environments.
Restrict them to reviewed release branches/tags and configure required reviewers
where supported by the repository's GitHub plan. Ordinary build jobs need no
signing secrets. Never expose release secrets to pull request code.

For each production channel, configure these environment variables:

- `STUDIO_RELEASE_NOTICES`: repository-relative path to a tracked, complete,
  reviewed dependency notice file for the shipped runtime and Rust dependencies.
- `STUDIO_RELEASE_NOTICES_SHA256`: SHA256 of that reviewed file.

The repository's `THIRD_PARTY_NOTICES.md` explicitly describes an incomplete
inventory; it is not sufficient by itself. The digest check prevents accidental
substitution, but does not perform the legal or completeness review.

For `microsoft-store`, also set the exact Partner Center values:

- `STUDIO_MSIX_IDENTITY_NAME`
- `STUDIO_MSIX_PUBLISHER`
- `STUDIO_MSIX_PUBLISHER_DISPLAY_NAME`

For `macos-dmg`, set variable `STUDIO_MACOS_SIGN_IDENTITY` to the full
`Developer ID Application: ...` identity. Add these environment secrets:

| Secret | Content |
| --- | --- |
| `APPLE_DEVELOPER_ID_P12_BASE64` | Base64 of the exported certificate and private key in a password-protected P12 |
| `APPLE_DEVELOPER_ID_P12_PASSWORD` | P12 export password |
| `APPLE_API_PRIVATE_KEY` | App Store Connect team API private key, including PEM headers |
| `APPLE_API_KEY_ID` | Key ID |
| `APPLE_API_ISSUER_ID` | Team API issuer ID |

Keep credentials out of Git and chat. The runner imports the identity into a
temporary keychain, validates notarization credentials and removes temporary
credential files. Cleanup runs on failure too. Only package outputs, lockfiles
and test evidence are uploaded; the runtime staging directory and keychain are
not included. GitHub runner cancellation or termination can interrupt cleanup;
use only ephemeral hosted runners with this workflow.

## Review the result

Download the artifact from the run's Summary page. Check `suite-package.json`
for the package hash and qualification status. Failed runs retain available test
evidence; failure during runner/environment setup may leave only GitHub job logs.

Test installation, startup, both tools, synthetic ROS reception, save/reopen,
exit/backend cleanup, update and uninstall on clean target machines. CI builds,
unit tests and notarization do not establish native UI, LAN or Store acceptance.
Do not submit a preview artifact as a qualified release.
