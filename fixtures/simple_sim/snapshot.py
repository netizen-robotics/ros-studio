"""Bounded, read-only fixture snapshots for the Docker-to-localhost test relay."""
import base64
from http.server import BaseHTTPRequestHandler, HTTPServer
import threading
from transport import encode_snapshot, MAX_CDR_BYTES, TOPICS

from rclpy.serialization import serialize_message


class Snapshot:
    def __init__(self):
        self.lock = threading.Lock()
        self.latest = {}
        self.sequence = 0
        self.payload = encode_snapshot({})
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path != '/snapshot':
                    self.send_error(404)
                    return
                with owner.lock:
                    payload = owner.payload
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                try:
                    self.wfile.write(payload)
                except (BrokenPipeError, ConnectionResetError):
                    pass

            def log_message(self, *_args):
                pass

            def setup(self):
                super().setup()
                self.connection.settimeout(2)

        self.server = HTTPServer(('0.0.0.0', 17877), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def publish(self, publisher, topic, typ, message):
        if topic not in TOPICS or typ != TOPICS[topic]:
            raise ValueError('Unexpected fixture topic/type')
        cdr = serialize_message(message)
        if len(cdr) > MAX_CDR_BYTES:
            raise ValueError('Fixture message exceeds its 16 KiB transport bound')
        with self.lock:
            sequence = self.sequence + 1
            candidate = {**self.latest, topic: dict(type=typ, sequence=sequence,
                                                   cdr=base64.b64encode(cdr).decode('ascii'))}
            payload = encode_snapshot(candidate)
            publisher.publish(message)
            self.sequence, self.latest, self.payload = sequence, candidate, payload

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)
