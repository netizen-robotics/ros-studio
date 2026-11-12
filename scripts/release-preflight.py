"""Validate release inputs without printing credentials or creating a package."""
import argparse
import hashlib
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def validate(channel, env):
    errors = []
    if channel in ('macos-preview', 'windows-setup'):
        return errors
    required = ['STUDIO_RELEASE_NOTICES', 'STUDIO_RELEASE_NOTICES_SHA256']
    if channel == 'macos-dmg':
        required += ['APPLE_DEVELOPER_ID_P12_BASE64', 'APPLE_DEVELOPER_ID_P12_PASSWORD',
                     'APPLE_API_PRIVATE_KEY', 'APPLE_API_KEY_ID', 'APPLE_API_ISSUER_ID',
                     'STUDIO_MACOS_SIGN_IDENTITY']
    elif channel == 'microsoft-store':
        required += ['STUDIO_MSIX_IDENTITY_NAME', 'STUDIO_MSIX_PUBLISHER',
                     'STUDIO_MSIX_PUBLISHER_DISPLAY_NAME']
    else:
        return ['Unsupported release channel']
    errors += [f'Missing {name}' for name in required if not env.get(name, '').strip()]
    if channel == 'macos-dmg' and env.get('STUDIO_MACOS_SIGN_IDENTITY') and not env['STUDIO_MACOS_SIGN_IDENTITY'].startswith('Developer ID Application: '):
        errors.append('DMG signing requires Developer ID Application, not Mac App Distribution')
    if env.get('STUDIO_RELEASE_NOTICES'):
        path = (ROOT / env['STUDIO_RELEASE_NOTICES']).resolve()
        if not path.is_relative_to(ROOT) or not path.is_file():
            errors.append('Reviewed notices must be a file within this checkout')
        else:
            tracked = subprocess.run(['git', 'ls-files', '--error-unmatch', '--', str(path.relative_to(ROOT))], cwd=ROOT, capture_output=True)
            if tracked.returncode:
                errors.append('Reviewed notices must be tracked in Git')
            if not path.stat().st_size or hashlib.sha256(path.read_bytes()).hexdigest() != env.get('STUDIO_RELEASE_NOTICES_SHA256'):
                errors.append('Reviewed notices SHA256 does not match the supplied file')
    return errors


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('channel', choices=['macos-preview', 'macos-dmg', 'microsoft-store', 'windows-setup'])
    errors = validate(parser.parse_args().channel, os.environ)
    for error in errors:
        print(error)
    if not errors:
        print('Release inputs present; this does not establish release qualification.')
    raise SystemExit(bool(errors))
