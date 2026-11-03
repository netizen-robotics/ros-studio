# Configure product signing

ROS Studio uses platform-owned distribution on Windows and Developer ID signing
on macOS. Platform signing does not change the repository copyright holder.
Keep credentials outside source, build artifacts and logs.

## macOS

Use an active Apple Developer Program organization membership. The account holder
creates a [Developer ID Application certificate](https://developer.apple.com/help/account/certificates/create-developer-id-certificates).
Keep its private key in the signing Mac's Keychain and confirm it appears in:

```sh
security find-identity -v -p codesigning
```

Store notarization credentials interactively, entering sensitive details only in
the local prompt:

```sh
xcrun notarytool store-credentials ros-studio-notary
export STUDIO_MACOS_NOTARY_PROFILE='ros-studio-notary'
export STUDIO_MACOS_SIGN_IDENTITY='Developer ID Application: YOUR LEGAL PUBLISHER (TEAMID)'
```

Follow Apple's [notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow).
The OS may show the certificate's legal publisher even when the app uses a shorter
brand. Run `scripts/check-signing.py` using the project Python, then pass the
identity and profile to `scripts/package-suite-macos.py` with reviewed dependency
notices. Production packaging signs the app and submits it for notarization.

## Windows Microsoft Store

Create a company developer account, reserve ROS Studio and obtain its package
identity from Partner Center. Set these non-secret values in the packaging shell:

```powershell
$env:STUDIO_MSIX_IDENTITY_NAME = 'EXACT_PACKAGE_IDENTITY_FROM_PARTNER_CENTER'
$env:STUDIO_MSIX_PUBLISHER = 'CN=EXACT_PUBLISHER_FROM_PARTNER_CENTER'
$env:STUDIO_MSIX_PUBLISHER_DISPLAY_NAME = 'YOUR_LEGAL_PUBLISHER'
$env:MAKEAPPX = (Get-Command makeappx.exe -ErrorAction Stop).Source
```

Run from a Windows SDK developer shell. The Store signs the MSIX after
certification; this route does not require buying a code-signing certificate.
See [Microsoft's signing options](https://learn.microsoft.com/windows/apps/package-and-deploy/code-signing-options).
Use Store builds of all frontends and submit the full suite as described in
[packaging](packaging.md). A Store package signature does not supply independent
Authenticode signatures for GitHub EXE distribution or privileged helpers.

## Read-only local checks

macOS:

```sh
scripts/dev .pixi/envs/default/bin/python scripts/check-signing.py
```

Windows, using the project environment:

```powershell
pixi run --locked python scripts/check-signing.py
```

Exit code 1 identifies missing local inputs. These checks never sign or upload,
read private keys, authenticate notarization, validate Store identity ownership or
prove clean-machine operation. Account enrollment, agreements, verification and
payment are completed directly by the account holder.
