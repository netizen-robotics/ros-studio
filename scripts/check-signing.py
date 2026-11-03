"""Read-only local signing configuration checks; never signs or publishes.

Use the project Python on the target host. Exit 1 means configuration is missing.
Passing these checks does not establish certificate trust or release qualification.
"""
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess


def command(*args):
    return subprocess.run(args, capture_output=True, text=True, timeout=30)


def main():
    checks = []

    def check(name, ok, detail):
        checks.append({'check': name, 'passed': bool(ok), 'detail': detail})

    if platform.system() == 'Darwin':
        identity = os.environ.get('STUDIO_MACOS_SIGN_IDENTITY', '')
        result = command('security', 'find-identity', '-v', '-p', 'codesigning')
        identities = re.findall(r'\b[0-9A-Fa-f]{40}\s+"([^"]+)"', result.stdout)
        check('Developer ID Application identity', result.returncode == 0
              and identity.startswith('Developer ID Application: ') and identity in identities,
              'Set STUDIO_MACOS_SIGN_IDENTITY to an available Developer ID Application identity with its private key')
        for tool in ('notarytool', 'stapler'):
            result = command('xcrun', '--find', tool)
            check(tool, result.returncode == 0, 'Required Apple command-line tool')
        check('Notarization profile name', os.environ.get('STUDIO_MACOS_NOTARY_PROFILE'),
              'STUDIO_MACOS_NOTARY_PROFILE names a local Keychain profile; credentials are not read or authenticated by this check')
    elif platform.system() == 'Windows':
        sdk = os.environ.get('MAKEAPPX') or shutil.which('makeappx.exe')
        check('Windows SDK MakeAppx', sdk and Path(sdk).is_file(), 'Set MAKEAPPX or use a Windows SDK developer shell')
        name = os.environ.get('STUDIO_MSIX_IDENTITY_NAME', '')
        check('Store package identity', re.fullmatch(r'[A-Za-z0-9.-]{3,50}', name), 'Use the exact reserved Partner Center identity; ownership is not verified locally')
        check('Store publisher', os.environ.get('STUDIO_MSIX_PUBLISHER', '').startswith('CN='), 'Use the exact publisher distinguished name from Partner Center')
        check('Publisher display name', os.environ.get('STUDIO_MSIX_PUBLISHER_DISPLAY_NAME'), 'Use the approved publisher display name')
    else:
        check('Supported signing host', False, 'Run on macOS or Windows')
    ready = all(c['passed'] for c in checks)
    print(json.dumps({'configuration_checks_passed': ready, 'checks': checks,
                      'limit': 'No signing, notarization authentication, certificate chain validation, purchase, upload or publication performed.'}, indent=2))
    return 0 if ready else 1


if __name__ == '__main__':
    raise SystemExit(main())
