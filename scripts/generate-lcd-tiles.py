#!/usr/bin/env python3
"""Regenerate the transparent, repeatable LCD colour-filter tiles (stdlib only)."""
from pathlib import Path
import struct
import zlib

OUT = Path(__file__).resolve().parent.parent / 'assets' / 'lcd'
COLORS = ((210, 91, 103), (83, 168, 109), (83, 125, 209))


def chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))


def tile(size):
    rows = []
    for _y in range(size):
        row = bytearray([0])  # PNG filter: none
        for x in range(size):
            # RGB stripe filter: no horizontal cell borders or CRT scanlines.
            # Only a faint gap between neighbouring RGB pixel triplets.
            if x == size - 1:
                rgba = (30, 49, 64, 5)
            else:
                channel = min(2, 3 * x // (size - 1))
                rgba = (*COLORS[channel], 10)
            row.extend(rgba)
        rows.append(row)
    return (b'\x89PNG\r\n\x1a\n'
            + chunk(b'IHDR', struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(b''.join(rows), 9))
            + chunk(b'IEND', b''))


if __name__ == '__main__':
    OUT.mkdir(parents=True, exist_ok=True)
    for size in (5, 6, 9):
        (OUT / f'rgb-cell-{size}.png').write_bytes(tile(size))
