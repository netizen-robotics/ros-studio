"""Diagnose Desktop host forwarding independently of DDS, using synthetic bytes."""
import json
import platform
import socket
import subprocess
import threading
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
(ROOT / 'artifacts/feasibility').mkdir(parents=True, exist_ok=True)
IMAGE = 'ros-studio-peer:jazzy'


def probe(transport, destination):
    kind = socket.SOCK_DGRAM if transport == 'udp' else socket.SOCK_STREAM
    received = []
    with socket.socket(socket.AF_INET, kind) as server:
        server.bind(('0.0.0.0', 0))
        server.settimeout(10)
        port = server.getsockname()[1]
        if transport == 'tcp':
            server.listen(1)
        def respond():
            try:
                if transport == 'udp':
                    data, address = server.recvfrom(128)
                    server.sendto(b'studio-response', address)
                else:
                    conn, _ = server.accept()
                    with conn:
                        conn.settimeout(8)
                        data = conn.recv(128)
                        conn.sendall(b'studio-response')
                received.append(data == b'studio-probe')
            except OSError as error:
                received.append(str(error))
        thread = threading.Thread(target=respond)
        thread.start()
        name = 'studio-network-' + uuid.uuid4().hex[:10]
        code = f'import socket; s=socket.socket(socket.AF_INET,socket.{"SOCK_DGRAM" if transport == "udp" else "SOCK_STREAM"}); s.settimeout(6); s.connect(({destination!r},{port})); s.sendall(b"studio-probe"); assert s.recv(128)==b"studio-response"; print("ROUNDTRIP_OK")'
        command = ['docker', 'run', '--rm', '--network', 'host', '--name', name, IMAGE, '/opt/fixture-venv/bin/python', '-c', code]
        try:
            result = subprocess.run(command, capture_output=True, text=True, timeout=15)
            output = result.stdout + result.stderr
            success = result.returncode == 0
        except subprocess.TimeoutExpired:
            output, success = 'Docker client timeout', False
        finally:
            subprocess.run(['docker', 'rm', '-f', name], capture_output=True, timeout=10)
            thread.join(timeout=11)
    return dict(transport=transport, destination=destination, bind='0.0.0.0', command=command,
                outcome='passed' if success and received == [True] else 'failed', output=output, native_received=received)


def main():
    image = json.loads(subprocess.check_output(['docker', 'image', 'inspect', IMAGE]))[0]
    records = []
    for transport in ['tcp', 'udp']:
        for destination in ['127.0.0.1', 'host.docker.internal']:
            record = probe(transport, destination)
            record.update(os=platform.platform(), cpu=platform.machine(), distro='jazzy', middleware_pair=None,
                          image_id=image['Id'], container_cpu=image['Architecture'], versions='evidence/docker-host/linux-packages.txt',
                          host_networking='enabled per user confirmation', scope='Transport diagnostic; no ROS communication assertion')
            records.append(record)
            print(transport, destination, record['outcome'], flush=True)
    (ROOT / 'artifacts/feasibility/docker-network-probe.json').write_text(json.dumps(records, indent=2) + '\n')
    return 0 if all(r['outcome'] == 'passed' for r in records) else 1


if __name__ == '__main__':
    raise SystemExit(main())
