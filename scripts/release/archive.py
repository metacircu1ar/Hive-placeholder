"""Stream pinned files into an archive and verify the bytes actually consumed."""
import hashlib
import io
import json
from pathlib import Path
import tarfile


class Payload:
    def __init__(self, source, mode, expected=None):
        self.source, self.mode = source, mode
        self.record = expected if expected is not None else {
            'sha256': hashlib.sha256(source).hexdigest(), 'bytes': len(source)}

    def manifest(self):
        return {key: self.record[key] for key in ('sha256', 'bytes')} | {'mode': self.mode}


class VerifiedReader:
    def __init__(self, stream):
        self.stream, self.digest, self.size = stream, hashlib.sha256(), 0

    def read(self, size=-1):
        data = self.stream.read(size)
        self.digest.update(data)
        self.size += len(data)
        return data

    def finish(self, expected):
        if self.read(1) or self.size != expected['bytes'] or self.digest.hexdigest() != expected['sha256']:
            raise ValueError('Consumed frozen file identity mismatch')


def write_archive(output, payloads):
    with tarfile.open(fileobj=output, mode='w:gz', format=tarfile.USTAR_FORMAT) as archive:
        for name, payload in sorted(payloads.items()):
            info = tarfile.TarInfo(name)
            info.size, info.mode, info.mtime = payload.record['bytes'], payload.mode, 0
            source = io.BytesIO(payload.source) if isinstance(payload.source, bytes) else Path(payload.source).open('rb')
            with source:
                reader = VerifiedReader(source)
                archive.addfile(info, reader)
                reader.finish(payload.record)


def json_bytes(value):
    return (json.dumps(value, sort_keys=True, indent=2) + '\n').encode()
