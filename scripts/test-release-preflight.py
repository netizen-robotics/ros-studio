"""Offline release-input checks; no signing credentials are used."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('preflight', Path(__file__).with_name('release-preflight.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ReleasePreflightTests(unittest.TestCase):
    def test_preview_needs_no_credentials(self):
        self.assertEqual(module.validate('macos-preview', {}), [])
        self.assertEqual(module.validate('windows-setup', {}), [])

    def test_production_does_not_fall_back_to_preview(self):
        self.assertIn('Missing APPLE_DEVELOPER_ID_P12_BASE64', module.validate('macos-dmg', {}))
        self.assertIn('Missing STUDIO_MSIX_PUBLISHER', module.validate('microsoft-store', {}))

    def test_signed_preview_requires_all_signing_inputs_but_not_review_attestation(self):
        errors = module.validate('macos-signed-preview', {})
        self.assertIn('Missing APPLE_API_PRIVATE_KEY', errors)
        self.assertNotIn('Missing STUDIO_RELEASE_NOTICES', errors)
        self.assertIn('Missing STUDIO_RELEASE_NOTICES', module.validate('macos-dmg', {}))

    def test_store_certificate_rejected_for_dmg_without_disclosing_value(self):
        errors = module.validate('macos-dmg', {'STUDIO_MACOS_SIGN_IDENTITY': '3rd Party Mac Developer Application: sentinel-sensitive-value'})
        self.assertTrue(any('requires Developer ID' in e for e in errors))
        self.assertNotIn('sentinel-sensitive-value', str(errors))

    def test_notices_cannot_escape_checkout(self):
        errors = module.validate('microsoft-store', {'STUDIO_RELEASE_NOTICES': '../outside.txt'})
        self.assertIn('Reviewed notices must be a file within this checkout', errors)

    def test_unsupported_channel_is_rejected(self):
        self.assertEqual(module.validate('mac-app-store', {}), ['Unsupported release channel'])


if __name__ == '__main__':
    unittest.main()
