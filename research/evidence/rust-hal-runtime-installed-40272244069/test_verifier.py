"""Offline comparison controls; never reads or changes original observations."""
import unittest

from verify import compare_result, require_original_matches


class OriginalResultControls(unittest.TestCase):
    def test_relocated_source_report_preserves_original_numeric_result(self):
        original = {'source_report': '/original/reader.json', 'pass': True, 'peak': 0.1}
        recomputed = {'source_report': '/download/reader.json', 'pass': True, 'peak': 0.1}
        exact = compare_result(recomputed, original)
        self.assertTrue(exact)
        require_original_matches({'reader': {'exact_original_result_except_source_path': exact}})

    def test_changed_numeric_result_cannot_pass_reproduction_gate(self):
        original = {'source_report': '/original/reader.json', 'pass': True, 'peak': 0.1}
        recomputed = {'source_report': '/download/reader.json', 'pass': True, 'peak': 0.2}
        exact = compare_result(recomputed, original)
        self.assertFalse(exact)
        with self.assertRaisesRegex(ValueError, 'differs from original observation'):
            require_original_matches({'reader': {'exact_original_result_except_source_path': exact}})

    def test_missing_observation_cannot_match(self):
        self.assertFalse(compare_result({'pass': True}, {'pass': True, 'peak': 0.0}))


if __name__ == '__main__':
    unittest.main()
