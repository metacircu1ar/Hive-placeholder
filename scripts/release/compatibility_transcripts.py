"""Replay frozen successful/negative envelopes against their producing binaries."""
import json
import socket

from .acceptance import require


def exchange(address, request):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(5)
        stream.connect(str(address))
        stream.sendall(json.dumps(request).encode() + b'\n')
        with stream.makefile('rb') as reader:
            line = reader.readline(1024 * 1024 + 1)
        require(len(line) <= 1024 * 1024 and line.endswith(b'\n'), 'Unbounded or incomplete corpus reply')
        return json.loads(line)


def expected(reply, pointers):
    for pointer, value in pointers.items():
        selected = reply
        for part in pointer.lstrip('/').split('/'):
            selected = selected[part.replace('~1', '/').replace('~0', '~')]
        require(selected == value, 'Frozen transcript expectation changed: ' + pointer)


def control(address, cases):
    for case in cases:
        expected(exchange(address, case['request']), case['expect'])


def service(address, connection, cases):
    credential = json.loads(connection.read_bytes())['credential']
    for case in cases:
        request = {'protocol': case['protocol'], 'credential': credential, 'claim': None, 'operation': case['operation']}
        expected(exchange(address, request), case['expect'])
