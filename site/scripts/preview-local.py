"""Local static preview on an OS-assigned loopback port; no cloud services."""
from functools import partial
from http.server import ThreadingHTTPServer, SimpleHTTPRequestHandler
from pathlib import Path
import argparse

parser = argparse.ArgumentParser()
parser.add_argument('--port', type=int, default=0)
args = parser.parse_args()

root = Path(__file__).resolve().parent.parent / 'public'
server = ThreadingHTTPServer(('127.0.0.1', args.port), partial(SimpleHTTPRequestHandler, directory=str(root)))
print(f'AIConnect preview: http://127.0.0.1:{server.server_port}/', flush=True)
server.serve_forever()
