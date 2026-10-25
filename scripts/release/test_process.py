"""Exit status wins over malformed stdout, and failed evidence is never success."""
import subprocess
import unittest
from unittest.mock import patch
from .process import ProcessFailure, json_reply
from .progress import finalizer, cleanup_all


class ProcessTests(unittest.TestCase):
    def test_all_cleanup_steps_run_and_first_failure_survives(self):
        seen = []
        primary, secondary = ValueError('first'), OSError('second')
        def fail(error):
            seen.append(error)
            raise error
        with patch(__package__ + '.progress.event') as report:
            with self.assertRaises(ValueError) as raised:
                cleanup_all(lambda: fail(primary), lambda: fail(secondary), lambda: seen.append('last'))
        self.assertIs(raised.exception, primary)
        self.assertEqual(seen, [primary, secondary, 'last'])
        self.assertEqual([call.kwargs['error'] for call in report.call_args_list], [primary, secondary])

    def test_nonzero_stdout_is_not_parsed_or_echoed(self):
        result = subprocess.CompletedProcess(['private-argument'], 17, 'SECRET_SENTINEL', 'private stderr')
        with patch(__package__ + '.process.json.loads', side_effect=AssertionError('must not parse')):
            with self.assertRaises(ProcessFailure) as raised:
                json_reply(result)
        self.assertIs(raised.exception.result, result)
        self.assertNotIn('SECRET_SENTINEL', str(raised.exception))
        self.assertIn('17', str(raised.exception))

    def test_success_requires_an_object_without_error(self):
        for stdout in ['not-json', '[]', '{"error": {"code": "failure"}}']:
            with self.assertRaises(RuntimeError):
                json_reply(subprocess.CompletedProcess([], 0, stdout, 'private stderr'))
        self.assertEqual(json_reply(subprocess.CompletedProcess([], 0, '{"ready": true}', '')), {'ready': True})

    def test_explicit_list_reply_preserves_exit_and_error_checks(self):
        rows = subprocess.CompletedProcess([], 0, '[{"slug":"inbox/note"}]', '')
        self.assertEqual(json_reply(rows, allow_list=True), [{'slug': 'inbox/note'}])
        with self.assertRaises(RuntimeError):
            json_reply(rows)
        for stdout in ['null', 'true', '"text"', '{"error":{"code":"failure"}}']:
            with self.assertRaises(RuntimeError):
                json_reply(subprocess.CompletedProcess([], 0, stdout, ''), allow_list=True)
        with self.assertRaises(ProcessFailure):
            json_reply(subprocess.CompletedProcess([], 1, '[]', ''), allow_list=True)

    def test_failed_cleanup_preserves_the_primary_object(self):
        primary = ValueError('primary')
        secondary = OSError('cleanup')
        def cleanup():
            raise secondary
        with patch(__package__ + '.progress.event') as report:
            with self.assertRaises(ValueError) as raised:
                with finalizer(cleanup):
                    raise primary
            self.assertIs(raised.exception, primary)
            self.assertIs(report.call_args.kwargs['error'], secondary)
            with self.assertRaises(OSError) as raised:
                with finalizer(cleanup):
                    pass
            self.assertIs(raised.exception, secondary)


if __name__ == '__main__':
    unittest.main()
