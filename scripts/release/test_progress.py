"""Exercise privacy, real blocked-descriptor exit and phase termination."""
import json
import io
import logging
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch
from .progress import Session, event, phase, exception_details


class ProgressTests(unittest.TestCase):
    def test_unavailable_diagnostics_fail_before_work_without_raw_traceback(self):
        from .progress import entry
        with patch(__package__ + '.progress.Session', side_effect=OSError('SECRET_SENTINEL')):
            with patch(__package__ + '.progress.event') as report:
                with patch('builtins.print') as output:
                    work = unittest.mock.Mock()
                    self.assertEqual(entry(work, name='build'), 1)
                    work.assert_not_called()
                    report.assert_not_called()
                    output.assert_not_called()

    def test_native_failure_retains_status_and_sizes_without_output(self):
        error = subprocess.CalledProcessError(9, ['SECRET_SENTINEL'],
                                              output=b'private stdout', stderr='private stderr')
        value = exception_details(error)[0]
        self.assertEqual(value['returncode'], 9)
        self.assertEqual(value['stdout_bytes'], 14)
        self.assertEqual(value['stderr_bytes'], 14)
        self.assertNotIn('private', json.dumps(value))
        self.assertNotIn('SECRET_SENTINEL', json.dumps(value))

    def test_helpers_outside_a_session_never_use_logging_last_resort(self):
        output = io.StringIO()
        handler = logging.StreamHandler(output)
        root = logging.getLogger()
        root.addHandler(handler)
        try:
            event('exception', error=RuntimeError('SECRET_SENTINEL'))
            self.assertEqual(output.getvalue(), '')
        finally:
            root.removeHandler(handler)
            handler.close()

    def test_exception_chain_and_library_records_never_format_private_text(self):
        with tempfile.TemporaryFile() as output:
            with Session(fd=output.fileno()) as session:
                try:
                    try:
                        raise OSError(28, 'SECRET_SENTINEL', '/private/account/secret')
                    except OSError as cause:
                        raise RuntimeError('SECRET_SENTINEL') from cause
                except RuntimeError as error:
                    event('exception', error=error, success=False)
                logging.getLogger('library').warning('SECRET_SENTINEL %s', '/private/account/secret')
                event('exception', payload='SECRET_SENTINEL')
            output.seek(0)
            data = output.read().decode()
            records = [json.loads(line) for line in data.splitlines()]
            self.assertNotIn('SECRET_SENTINEL', data)
            self.assertNotIn('/private/account', data)
            self.assertEqual([c['type'] for c in records[0]['causes']], ['RuntimeError', 'OSError'])
            self.assertEqual(records[0]['causes'][1]['errno'], 28)
            self.assertTrue(records[0]['causes'][0]['frames'])
            self.assertIn('tools.library_record', [r['event'] for r in records])
            self.assertEqual(session.finish()['invalid'], 1)

    def test_heartbeat_stops_and_records_a_failed_phase(self):
        with tempfile.TemporaryFile() as output:
            with Session(fd=output.fileno()):
                with self.assertRaises(ValueError):
                    with phase('build', interval=0.02):
                        time.sleep(0.08)
                        raise ValueError('private')
                time.sleep(0.06)
            output.seek(0)
            records = [json.loads(line) for line in output.read().splitlines()]
        self.assertEqual(records[0]['event'], 'tools.phase_started')
        self.assertTrue(any(r['event'] == 'tools.heartbeat' for r in records))
        self.assertEqual(records[-1]['event'], 'tools.phase_finished')
        self.assertFalse(records[-1]['success'])

    def test_unread_stderr_does_not_block_actual_interpreter_shutdown(self):
        code = '''
import fcntl,json,os,time
from scripts.release.progress import Session,event
flags = fcntl.fcntl(2, fcntl.F_GETFL)
fcntl.fcntl(2, fcntl.F_SETFL, flags | os.O_NONBLOCK)
try:
    while True: os.write(2, b'x' * 4096)
except BlockingIOError: pass
finally: fcntl.fcntl(2, fcntl.F_SETFL, flags)
with Session(slots=4) as session:
    event('phase_started', phase='build')
    time.sleep(0.05)
    for _ in range(1000): event('heartbeat', phase='build', elapsed_ms=10)
    result = session.finish(0.05)
print(json.dumps(result))
'''
        process = subprocess.Popen([sys.executable, '-c', code], cwd=Path(__file__).resolve().parents[2],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            # Intentionally do not drain stderr until the interpreter has exited.
            self.assertEqual(process.wait(timeout=4), 0)
            out, _ = process.communicate(timeout=1)
            result = json.loads(out)
            self.assertFalse(result['confirmed'])
            self.assertGreater(result['dropped'], 0)
        finally:
            if process.poll() is None:
                process.kill()
            process.communicate()

    def test_cli_entry_does_not_print_python_default_traceback(self):
        code = "from scripts.release.progress import entry; import sys; def_placeholder"
        code = code.replace('def_placeholder', "\ndef fail(): raise RuntimeError('SECRET_SENTINEL')\nsys.exit(entry(fail, name='build'))")
        output = subprocess.run([sys.executable, '-c', code], cwd=Path(__file__).resolve().parents[2],
                                capture_output=True, text=True, timeout=4, check=False)
        self.assertEqual(output.returncode, 1)
        self.assertEqual(output.stdout, '')
        self.assertNotIn('SECRET_SENTINEL', output.stderr)
        self.assertNotIn('Traceback (most recent call last)', output.stderr)
        self.assertIn('RuntimeError', output.stderr)


if __name__ == '__main__':
    unittest.main()
