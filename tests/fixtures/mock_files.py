"""Load the product helper in source tests; copied fixtures get the helper itself."""
import importlib.util
from pathlib import Path

_path = Path(__file__).resolve().parents[2] / 'scripts/release/files.py'
_spec = importlib.util.spec_from_file_location('hive_mock_files', _path)
_module = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_module)
atomic_write_text = _module.atomic_write_text
atomic_write_json = _module.atomic_write_json
atomic_copy = _module.atomic_copy
append_record = _module.append_record
append_jsonl_record = _module.append_jsonl_record
record_lock = _module.record_lock
CompleteLines = _module.CompleteLines
AppendStream = _module.AppendStream
