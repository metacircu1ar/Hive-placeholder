import unittest

from .acceptance import terminal_json


class TerminalResponseTests(unittest.TestCase):
    def test_diagnostic_objects_do_not_replace_the_command_response(self):
        output = (
            b'Hive info auth.login phase="started"\r\n'
            b'Hive warn cleanup.secondary_failed error={"kind":"io"}\r\n'
            b'{\r\n  "state": "authenticated"\r\n}\r\n'
        )
        self.assertEqual(terminal_json(output), {'state': 'authenticated'})

    def test_missing_incomplete_and_multiple_responses_are_not_accepted(self):
        for output in [b'Hive info auth.login\r\n', b'{"state":', b'{}\r\n{}\r\n']:
            with self.subTest(output=output), self.assertRaises(ValueError):
                terminal_json(output)
