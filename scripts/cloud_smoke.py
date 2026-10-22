#!/usr/bin/env python3
"""Synthetic provider contract check. Uses env credentials; sends no clipboard data."""
import base64
import json
import math
import os
import struct
import urllib.error
import urllib.request
import zlib


def png(rgb):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    pixels = (b"\0" + bytes(rgb) * 64) * 64
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 64, 64, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(pixels)) + chunk(b"IEND", b"")


def main():
    key = os.environ.get("OPENROUTER_API_KEY", "")
    if not key:
        raise SystemExit("OPENROUTER_API_KEY is missing")
    body = {
        "model": "google/gemini-embedding-2",
        "input": [
            {"content": [{"type": "text", "text": "a solid red square"}]},
            {"content": [{"type": "image_url", "image_url": {"url": "data:image/png;base64," + base64.b64encode(png((255, 0, 0))).decode()}}]},
        ],
        "encoding_format": "float",
    }
    request = urllib.request.Request("https://openrouter.ai/api/v1/embeddings", data=json.dumps(body).encode(), headers={"Authorization": "Bearer " + key, "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            result = json.load(response)
    except urllib.error.HTTPError as exc:
        raise SystemExit(f"Provider returned HTTP {exc.code}; no private response body printed") from None
    vectors = [item["embedding"] for item in sorted(result["data"], key=lambda x: x["index"])]
    assert len(vectors) == 2 and len(vectors[0]) == len(vectors[1])
    assert all(math.isfinite(v) for row in vectors for v in row)
    norms = [math.sqrt(sum(v*v for v in row)) for row in vectors]
    assert all(n > 0 for n in norms)
    cosine = sum(a*b for a, b in zip(*vectors)) / (norms[0] * norms[1])
    print(f"PASS: native text/image embeddings, dimensions={len(vectors[0])}, cosine={cosine:.4f}")


if __name__ == "__main__":
    main()
