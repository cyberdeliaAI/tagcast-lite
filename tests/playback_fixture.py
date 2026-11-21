"""Generate synthetic audio and run the native player checks on a private HTTP fixture."""

import argparse
import array
import collections
import contextlib
import http.server
import json
import math
import pathlib
import re
import socket
import subprocess
import tempfile
import threading
import time
import urllib.parse
import wave

RATE = 48_000
SECONDS = 16


def fixtures(folder):
    samples = array.array("h", (
        round(11_000 * math.sin(2 * math.pi * (220 + index // RATE * 45) * index / RATE))
        for index in range(RATE * SECONDS)
    ))
    stereo = array.array("h", (sample for sample in samples for _ in range(2)))
    with wave.open(str(folder / "tone.wav"), "wb") as out:
        out.setparams((2, 2, RATE, 0, "NONE", ""))
        out.writeframes(stereo.tobytes())
    for args in (
        ["-c:a", "flac", "tone.flac"],
        ["-ar", "44100", "-c:a", "flac", "resample.flac"],
        ["-c:a", "libmp3lame", "-b:a", "128k", "tone.mp3"],
        ["-c:a", "aac", "-b:a", "128k", "-f", "hls", "-hls_time", "2",
         "-hls_playlist_type", "vod", "-hls_segment_filename", "segment-%02d.ts", "tone.m3u8"],
    ):
        subprocess.run(["ffmpeg", "-nostdin", "-loglevel", "error", "-i", "tone.wav", *args],
                       cwd=folder, check=True)

    subprocess.run([
        "ffmpeg", "-nostdin", "-loglevel", "error", "-i", "resample.flac",
        "-i", "resample.flac", "-filter_complex", "concat=n=2:v=0:a=1",
        "-c:a", "flac", "joined.flac",
    ], cwd=folder, check=True)


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def handle(self):
        with contextlib.suppress(BrokenPipeError, ConnectionResetError):
            super().handle()

    def do_GET(self):
        path = urllib.parse.urlsplit(self.path).path.lstrip("/")
        if path == "stats":
            with self.server.lock:
                data = json.dumps(self.server.log).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return

        with self.server.lock:
            self.server.log.append([path, self.headers.get("Range")])
            self.server.count[path] += 1
            attempt = self.server.count[path]
        try:
            self.serve(path, attempt)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def serve(self, path, attempt):
        if path in ("missing.mp3", "missing.m3u8") or path == "once.mp3" and attempt == 1:
            self.send_error(503 if path == "once.mp3" else 404)
            return
        if path == "always.mp3":
            self.send_error(503)
            return
        if path == "foreign.m3u8":
            body = b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\nhttp://127.0.0.1:1/private.ts\n#EXT-X-ENDLIST\n"
        elif path == "unsupported.orig":
            body = b"Unknown audio format" * 40
        elif path == "slow.mp3":
            time.sleep(3)
            body = self.server.assets["tone.mp3"]
        elif path == "event.m3u8":
            source = self.server.assets["tone.m3u8"].decode().replace(":VOD", ":EVENT")
            if attempt == 1:
                lines = source.splitlines()
                idx = next(i for i, line in enumerate(lines) if line.endswith(".ts"))
                body = ("\n".join(lines[:idx + 1]) + "\n").encode()
            else:
                body = source.encode()
        elif path in {"resample.flac", "joined.flac"}:
            body = self.server.assets[path]
        elif path == "latency-hls.m3u8":
            body = self.server.assets["tone.m3u8"]
        elif path.startswith("segment-") or path.endswith(".m3u8"):
            body = self.server.assets[path]
        else:
            body = self.server.assets[f"tone.{path.rsplit('.', 1)[1]}"]

        if path.startswith("chunked"):
            self.send_response(200)
            self.send_header("Transfer-Encoding", "chunked")
            self.send_header("Content-Type", "audio/mpeg")
            self.end_headers()
            for start in range(0, len(body), 8192):
                chunk = body[start:start + 8192]
                self.wfile.write(f"{len(chunk):x}\r\n".encode() + chunk + b"\r\n")
                if path == "chunked-slow.mp3":
                    time.sleep(0.06)
            self.wfile.write(b"0\r\n\r\n")
            return

        header = self.headers.get("Range")
        is_audio = path.endswith((".mp3", ".flac"))
        offset = 0
        if is_audio and header:
            match = re.fullmatch(r"bytes=(\d+)-", header)
            if not match or int(match[1]) >= len(body):
                self.send_error(416)
                return
            offset = int(match[1])
            self.send_response(206)
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("Content-Range", f"bytes {offset}-{len(body) - 1}/{len(body)}")
        else:
            self.send_response(200)
        self.send_header("Content-Length", str(len(body) - offset))
        self.send_header("Content-Type", {
            "mp3": "audio/mpeg", "flac": "audio/flac", "m3u8": "application/vnd.apple.mpegurl",
            "ts": "video/mp2t",
        }.get(path.rsplit(".", 1)[1], "application/octet-stream"))
        if path.startswith("latency-"):
            time.sleep(0.040)
        self.end_headers()
        if path == "drop.flac" and attempt == 1:
            self.wfile.write(body[offset:offset + 128_000])
            self.wfile.flush()
            time.sleep(0.15)
            self.connection.shutdown(socket.SHUT_RDWR)
            self.close_connection = True
            return
        self.wfile.write(body[offset:])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("example", type=pathlib.Path)
    parser.add_argument("--soak", type=int, default=120)
    parser.add_argument("--synthetic-hours", type=float, default=1)
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--skip-device", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="tagcast-player-fixture-") as tmp:
        folder = pathlib.Path(tmp)
        fixtures(folder)
        with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
            server.daemon_threads = True
            server.log = []
            server.count = collections.Counter()
            server.lock = threading.Lock()
            server.assets = {p.name: p.read_bytes() for p in folder.iterdir() if p.suffix != ".wav"}
            worker = threading.Thread(target=server.serve_forever, daemon=True)
            worker.start()
            try:
                base = f"http://127.0.0.1:{server.server_port}"
                command = [str(args.example.resolve()), base, str(args.soak), str(args.synthetic_hours)]
                if args.skip_device:
                    command.append("--skip-device")
                proc = subprocess.run(command, capture_output=True, text=True)
                if proc.stderr:
                    print(proc.stderr, end="")
                if proc.returncode:
                    print(proc.stdout, end="")
                    raise SystemExit(proc.returncode)
                report = json.loads(proc.stdout)
                report["fixture_requests"] = dict(sorted(server.count.items()))
                report["verified_nonzero_http_ranges"] = sum(
                    bool(header and header != "bytes=0-") for _, header in server.log)
                assert report["verified_nonzero_http_ranges"] > 0
                assert server.count["always.mp3"] == 2
                assert server.count["once.mp3"] == 2
                assert server.count["missing.mp3"] == 1
                assert server.count["drop.flac"] > 1
                assert server.count["chunked-slow.mp3"] == 1
                encoded = json.dumps(report, indent=2) + "\n"
                if args.output:
                    args.output.write_text(encoded)
                print(encoded, end="")
            finally:
                with contextlib.suppress(OSError):
                    server.shutdown()
                worker.join()


if __name__ == "__main__":
    main()
