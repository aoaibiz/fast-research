"""Minimal QR Code encoder (byte mode, error correction M, versions 1-10) rendered as SVG.

Follows the structure of the public QR Code specification (ISO/IEC 18004) the same way
Project Nayuki's MIT-licensed qrcodegen does; no external dependency, no network.
"""

_ECC_PER_BLOCK_M = (None, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26)
_BLOCKS_M = (None, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5)
_FORMAT_M = 0  # format bits for error correction level M
_MASKS = (
    lambda x, y: (x + y) % 2 == 0,
    lambda x, y: y % 2 == 0,
    lambda x, y: x % 3 == 0,
    lambda x, y: (x + y) % 3 == 0,
    lambda x, y: (x // 3 + y // 2) % 2 == 0,
    lambda x, y: x * y % 2 + x * y % 3 == 0,
    lambda x, y: (x * y % 2 + x * y % 3) % 2 == 0,
    lambda x, y: ((x + y) % 2 + x * y % 3) % 2 == 0,
)


def _raw_modules(ver):
    result = (16 * ver + 128) * ver + 64
    if ver >= 2:
        align = ver // 7 + 2
        result -= (25 * align - 10) * align - 55
        if ver >= 7:
            result -= 36
    return result


def _data_codewords(ver):
    return _raw_modules(ver) // 8 - _ECC_PER_BLOCK_M[ver] * _BLOCKS_M[ver]


def _gf_mul(x, y):
    z = 0
    for i in reversed(range(8)):
        z = (z << 1) ^ ((z >> 7) * 0x11D)
        z ^= ((y >> i) & 1) * x
    return z


def _rs_divisor(degree):
    result = [0] * (degree - 1) + [1]
    root = 1
    for _ in range(degree):
        for j in range(degree):
            result[j] = _gf_mul(result[j], root)
            if j + 1 < degree:
                result[j] ^= result[j + 1]
        root = _gf_mul(root, 0x02)
    return result


def _rs_remainder(data, divisor):
    result = [0] * len(divisor)
    for b in data:
        factor = b ^ result.pop(0)
        result.append(0)
        for i, coef in enumerate(divisor):
            result[i] ^= _gf_mul(coef, factor)
    return result


def _alignment_positions(ver):
    if ver == 1:
        return []
    count = ver // 7 + 2
    step = (ver * 4 + count * 2 + 1) // (count * 2 - 2) * 2
    size = ver * 4 + 17
    result = []
    pos = size - 7
    for _ in range(count - 1):
        result.insert(0, pos)
        pos -= step
    return [6] + result


def matrix(text):
    """Return the QR matrix (list of rows of bools, True = dark) for the UTF-8 text."""
    data = text.encode('utf-8')
    for ver in range(1, 11):
        count_bits = 8 if ver < 10 else 16
        if 4 + count_bits + 8 * len(data) <= _data_codewords(ver) * 8:
            break
    else:
        raise ValueError('qr_too_long')
    capacity = _data_codewords(ver) * 8
    bits = []
    def put(value, length):
        bits.extend((value >> i) & 1 for i in reversed(range(length)))
    put(0b0100, 4); put(len(data), count_bits)
    for b in data:
        put(b, 8)
    put(0, min(4, capacity - len(bits)))
    put(0, (-len(bits)) % 8)
    pad = 0xEC
    while len(bits) < capacity:
        put(pad, 8); pad ^= 0xEC ^ 0x11
    codewords = [int(''.join(map(str, bits[i:i + 8])), 2) for i in range(0, len(bits), 8)]

    # Split into blocks, add Reed-Solomon error correction and interleave.
    blocks_n, ecc_len = _BLOCKS_M[ver], _ECC_PER_BLOCK_M[ver]
    raw = _raw_modules(ver) // 8
    short_n = blocks_n - raw % blocks_n
    short_len = raw // blocks_n
    divisor = _rs_divisor(ecc_len)
    blocks, k = [], 0
    for i in range(blocks_n):
        dat = codewords[k:k + short_len - ecc_len + (0 if i < short_n else 1)]
        k += len(dat)
        ecc = _rs_remainder(dat, divisor)
        if i < short_n:
            dat = dat + [0]
        blocks.append(dat + ecc)
    final = []
    for i in range(len(blocks[0])):
        for j, block in enumerate(blocks):
            if i != short_len - ecc_len or j >= short_n:
                final.append(block[i])

    size = ver * 4 + 17
    modules = [[False] * size for _ in range(size)]
    function = [[False] * size for _ in range(size)]
    def setf(x, y, dark):
        modules[y][x] = dark; function[y][x] = True
    for i in range(size):
        setf(6, i, i % 2 == 0); setf(i, 6, i % 2 == 0)
    for cx, cy in ((3, 3), (size - 4, 3), (3, size - 4)):
        for dy in range(-4, 5):
            for dx in range(-4, 5):
                x, y = cx + dx, cy + dy
                if 0 <= x < size and 0 <= y < size:
                    setf(x, y, max(abs(dx), abs(dy)) not in (2, 4))
    positions = _alignment_positions(ver)
    last = len(positions) - 1
    for i, ay in enumerate(positions):
        for j, ax in enumerate(positions):
            if (i, j) in ((0, 0), (0, last), (last, 0)):
                continue
            for dy in range(-2, 3):
                for dx in range(-2, 3):
                    setf(ax + dx, ay + dy, max(abs(dx), abs(dy)) != 1)
    def draw_format(mask):
        value = _FORMAT_M << 3 | mask
        rem = value
        for _ in range(10):
            rem = (rem << 1) ^ ((rem >> 9) * 0x537)
        fbits = (value << 10 | rem) ^ 0x5412
        bit = lambda i: (fbits >> i) & 1 == 1
        for i in range(6):
            setf(8, i, bit(i))
        setf(8, 7, bit(6)); setf(8, 8, bit(7)); setf(7, 8, bit(8))
        for i in range(9, 15):
            setf(14 - i, 8, bit(i))
        for i in range(8):
            setf(size - 1 - i, 8, bit(i))
        for i in range(8, 15):
            setf(8, size - 15 + i, bit(i))
        setf(8, size - 8, True)
    draw_format(0)
    if ver >= 7:
        rem = ver
        for _ in range(12):
            rem = (rem << 1) ^ ((rem >> 11) * 0x1F25)
        vbits = ver << 12 | rem
        for i in range(18):
            dark = (vbits >> i) & 1 == 1
            a, b = size - 11 + i % 3, i // 3
            setf(a, b, dark); setf(b, a, dark)

    i, right = 0, size - 1
    while right >= 1:
        if right == 6:
            right = 5
        for vert in range(size):
            for j in range(2):
                x = right - j
                upward = ((right + 1) & 2) == 0
                y = size - 1 - vert if upward else vert
                if not function[y][x] and i < len(final) * 8:
                    modules[y][x] = (final[i >> 3] >> (7 - (i & 7))) & 1 == 1
                    i += 1
        right -= 2

    def masked(mask):
        rule = _MASKS[mask]
        return [[modules[y][x] ^ (not function[y][x] and rule(x, y)) for x in range(size)] for y in range(size)]
    best, best_score = None, None
    for mask in range(8):
        draw_format(mask)
        candidate = masked(mask)
        score = _penalty(candidate)
        if best_score is None or score < best_score:
            best, best_score = candidate, score
    return best


def _penalty(grid):
    size = len(grid)
    score = 0
    lines = grid + [list(col) for col in zip(*grid)]
    for line in lines:
        run, prev = 0, None
        for cell in line:
            if cell == prev:
                run += 1
            else:
                if run >= 5:
                    score += 3 + run - 5
                run, prev = 1, cell
        if run >= 5:
            score += 3 + run - 5
        text = ''.join('1' if c else '0' for c in line)
        padded = '0000' + text + '0000'
        for pattern in ('10111010000', '00001011101'):
            start = padded.find(pattern)
            while start != -1:
                score += 40
                start = padded.find(pattern, start + 1)
    for y in range(size - 1):
        for x in range(size - 1):
            if grid[y][x] == grid[y][x + 1] == grid[y + 1][x] == grid[y + 1][x + 1]:
                score += 3
    dark = sum(map(sum, grid))
    total = size * size
    score += max((abs(dark * 20 - total * 10) + total - 1) // total - 1, 0) * 10
    return score


def svg(text, border=4):
    grid = matrix(text)
    size = len(grid) + border * 2
    path = ''.join(f'M{x + border} {y + border}h1v1h-1z' for y, row in enumerate(grid) for x, dark in enumerate(row) if dark)
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" shape-rendering="crispEdges">'
            f'<rect width="{size}" height="{size}" fill="#fff"/><path fill="#000" d="{path}"/></svg>')
