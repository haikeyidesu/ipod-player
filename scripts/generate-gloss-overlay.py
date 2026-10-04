#!/usr/bin/env python3
"""Generate a transparent 2x clear-coat for the 420x640 Slint device geometry."""
from math import exp, hypot
from pathlib import Path
import struct
import zlib

OUT = Path(__file__).resolve().parent.parent / 'assets/gloss/reflection-overlay.png'
SCALE = 2
WIDTH, HEIGHT = 420 * SCALE, 640 * SCALE


def smoothstep(a, b, value):
    t = max(0.0, min(1.0, (value - a) / (b - a)))
    return t * t * (3 - 2 * t)


def rounded_distance(x, y, left, top, width, height, radius):
    dx = abs(x - left - width / 2) - (width / 2 - radius)
    dy = abs(y - top - height / 2) - (height / 2 - radius)
    return hypot(max(dx, 0), max(dy, 0)) + min(max(dx, dy), 0) - radius


def gaussian(value):
    return exp(-value * value)


def chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))


def pixel(x, y):
    shell_d = rounded_distance(x, y, 0, 0, 420, 640, 32)
    shell = smoothstep(-0.6, 0.6, -shell_d)
    if shell == 0:
        return (255, 255, 255, 0)

    # Entire 230px wheel, including its rim, is clear. Feather only OUTSIDE it.
    wheel_clear = smoothstep(117, 120, hypot(x - 210, y - 471))
    if wheel_clear == 0:
        return (255, 255, 255, 0)

    bezel_d = rounded_distance(x, y, 50, 32, 320, 300, 10)
    lcd_d = rounded_distance(x, y, 57, 39, 306, 286, 4)
    shell_area = smoothstep(-1, 1, bezel_d)
    lcd_area = smoothstep(-0.6, 0.6, -lcd_d)

    # White polycarbonate clear-coat: corner catchlights, curved side ribbons,
    # a broad upper-left glow and a translucent diagonal wash. No base fill.
    upper_left = 39 * gaussian((x - 79) / 140) * gaussian((y - 55) / 150)
    left_edge = 104 * gaussian((x - 13) / 10) * gaussian((y - 293) / 360)
    right_edge = 95 * gaussian((x - 407) / 10) * gaussian((y - 330) / 385)
    top_edge = 76 * gaussian((shell_d + 7) / 6) * (1 - smoothstep(40, 250, y))
    bottom_edge = 49 * gaussian((shell_d + 7) / 8) * smoothstep(450, 630, y)
    diagonal = 25 * gaussian((y - 0.88 * x - 35) / 100)
    body = shell_area * min(150, upper_left + left_edge + right_edge
                            + top_edge + bottom_edge + diagonal)

    # The LCD stays clear around labels; the upper/right curve catches a
    # little light, with a whisper of diagonal reflection elsewhere.
    corner = 16 * gaussian((x - 364) / 72) * gaussian((y - 47) / 95)
    screen_diagonal = 4 * gaussian((y + 0.72 * x - 340) / 43)
    screen = lcd_area * (1.0 + corner + screen_diagonal)
    alpha = round(shell * wheel_clear * min(255, body + screen))
    return (247, 252, 255, alpha)


def main():
    compressor = zlib.compressobj(level=9)
    data = bytearray()
    for row in range(HEIGHT):
        raw = bytearray([0])  # PNG filter: none
        for col in range(WIDTH):
            raw.extend(pixel((col + 0.5) / SCALE, (row + 0.5) / SCALE))
        data.extend(compressor.compress(raw))
    data.extend(compressor.flush())
    png = (b'\x89PNG\r\n\x1a\n'
           + chunk(b'IHDR', struct.pack('>IIBBBBB', WIDTH, HEIGHT, 8, 6, 0, 0, 0))
           + chunk(b'IDAT', bytes(data)) + chunk(b'IEND', b''))
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(png)
    print(f'Wrote {OUT} ({WIDTH}x{HEIGHT}, RGBA)')


if __name__ == '__main__':
    main()
