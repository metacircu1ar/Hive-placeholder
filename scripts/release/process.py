"""Interpret subprocess outcomes before parsing their structured stdout."""
import json
from .progress import event


class ProcessFailure(RuntimeError):
    def __init__(self, result):
        super().__init__(f'CLI process failed with exit status {result.returncode}')
        # Keep private evidence available to explicit callers, never in log messages.
        self.result = result


def byte_count(value):
    if isinstance(value, bytes):
        return len(value)
    return len(value.encode('utf-8', errors='replace')) if value is not None else 0


def checked(result):
    summary = dict(returncode=result.returncode, stderr_bytes=byte_count(result.stderr),
                   stdout_bytes=byte_count(result.stdout))
    if result.returncode != 0:
        event('subprocess_failed', **summary)
        raise ProcessFailure(result)
    return result


def json_reply(result, *, allow_list=False):
    checked(result)
    summary = dict(returncode=result.returncode, stderr_bytes=byte_count(result.stderr),
                   stdout_bytes=byte_count(result.stdout))
    try:
        value = json.loads(result.stdout)
    except (ValueError, TypeError) as error:
        event('subprocess_invalid_json', error=error, **summary)
        raise RuntimeError('Successful CLI process returned invalid JSON') from error
    if not isinstance(value, dict) and not (allow_list and isinstance(value, list)):
        event('subprocess_invalid_json', **summary)
        raise RuntimeError('Successful CLI process returned an unexpected response shape')
    if isinstance(value, dict) and 'error' in value:
        event('subprocess_failed', **summary)
        raise RuntimeError('CLI reported an error despite a successful exit status')
    return value
