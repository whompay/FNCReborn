"""Adds a "DISPLAY" tab to Fight Night Champion's Settings screen.

Patches the Apt movie gamesettingdata.big inside dist\\game\\databigs\\xenon\\fe.big:
  * appends new strings to the Apt constant file (items array moved to the end),
  * copies the movie's frame-0 action to the end of the Apt data file, with a bigger
    ConstantPool (old 306 entries + ours) and our code before the final End,
    then points frame 0 at the copy (old bytes are left untouched),
  * re-packs the BIGF (members RefPack-compressed: the game decompresses in place, so a
    member must not be bigger compressed than uncompressed) and appends it
    to fe.big, repointing that file's 16-byte table entry.
Values for the new rows are served natively by project\\src\\fnc_settings_menu.cpp
(category id 200, setting ids 201+; keep both files in sync).

Usage:  python aptpatch_display.py <path to fe.big>
Creates <fe.big>.orig on first run and always patches from it, so re-running is safe.
Restore the original: copy fe.big.orig over fe.big.
"""
import os, shutil, struct, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ebbig import read_index
from refpack import unrefpack
from refpack_enc import refpack

CATEGORY = 200
TAB_NAME = 'DISPLAY'
ROWS = [  # (title, labels) -> setting ids 201, 202, ... ; must match kRows in fnc_settings_menu.cpp
    ('FRAME RATE (RESTART)', ['30 FPS', '60 FPS', '120 FPS', '144 FPS', '165 FPS', '180 FPS', '240 FPS']),
    ('VSYNC', ['OFF', 'ON']),
    ('RESOLUTION SCALE (RESTART)', ['1X', '2X', '3X']),
    ('ANTI-ALIASING', ['OFF', 'FXAA', 'FXAA EXTREME']),
    ('FULLSCREEN', ['OFF', 'ON']),
]
CONTEXTS = ['FROM_MAIN_MENU', 'FROM_PAUSE_MENU', 'FROM_MOVIE_PAUSE_MENU', 'FROM_TRAINING_PAUSE_MENU',
            'FROM_PLAY_NOW_FLOW', 'FROM_LEGACY', 'FROM_AMATEUR_TOURNAMENT']
MEMBER = 'gamesettingdata.big'
COPY_ONLY = '--copy-only' in sys.argv  # testing: relocate the stream but add nothing


def u32(b, o): return struct.unpack('>I', b[o:o + 4])[0]
def p32(v): return struct.pack('>I', v)


# ---------------------------------------------------------------- RefPack (literals only)
def refpack_store(data):
    out = bytearray(b'\x10\xfb' + len(data).to_bytes(3, 'big'))
    p = 0
    while len(data) - p >= 4:
        n = min(112, (len(data) - p) & ~3)
        out.append(0xE0 | ((n - 4) >> 2)); out += data[p:p + n]; p += n
    rest = len(data) - p
    out.append(0xFC | rest); out += data[p:]
    assert unrefpack(bytes(out)) == bytes(data)
    return bytes(out)


# ---------------------------------------------------------------- BIGF
def bigf_read(b):
    n = u32(b, 8); p = 16; mem = []
    for _ in range(n):
        off, size = struct.unpack('>II', b[p:p + 8]); p += 8
        e = b.index(b'\0', p); nm = b[p:e].decode(); p = e + 1
        mem.append([nm, b[off:off + size]])
    footer = b[p:u32(b, 12)]
    return mem, footer


def bigf_write(mem, footer, align=0x40):
    dir_len = 16 + sum(8 + len(nm) + 1 for nm, _ in mem)
    hdr_size = dir_len + len(footer)
    pos = (hdr_size + align - 1) // align * align
    offs = []
    for nm, data in mem:
        offs.append(pos)
        if data: pos = (pos + len(data) + align - 1) // align * align
    total = offs[-1] + len(mem[-1][1])
    h = bytearray(b'BIGF' + struct.pack('<I', total) + p32(len(mem)) + p32(hdr_size))
    for (nm, data), off in zip(mem, offs):
        h += p32(off) + p32(len(data)) + nm.encode() + b'\0'
    h += footer
    out = bytearray(h) + bytes(offs[0] - len(h))
    for (nm, data), off in zip(mem, offs):
        out += bytes(off - len(out)) + data
    return bytes(out[:total])


# ---------------------------------------------------------------- Apt patch
class Asm:
    def __init__(self, base_off, pool_index):
        self.b = bytearray(); self.base = base_off; self.ix = pool_index

    def pos(self): return self.base + len(self.b)
    def op(self, o, *operand): self.b.append(o); self.b += bytes(operand)

    def align(self):
        while self.pos() % 4: self.b.append(0)

    def const(self, s):  # push a pooled string
        i = self.ix(s)
        if i < 256: self.op(0xA2, i)
        else: self.b.append(0xA3); self.b += struct.pack('>H', i)

    def byte_op(self, o, s):  # ops that take a byte pool index
        i = self.ix(s); assert i < 256, s; self.op(o, i)

    def var(self, s): self.byte_op(0xAE, s)          # PushValueOfVar
    def member(self, s): self.byte_op(0xAF, s)       # GetNamedMember
    def int_(self, v):
        if v == 0: self.op(0x59)
        elif v == 1: self.op(0x5A)
        elif v < 128: self.op(0xB5, v)
        else: self.b.append(0xB6); self.b += struct.pack('>H', v)

    def function(self, body_fn, name_ptr, params_ptr):  # anonymous DefineFunction (0x9B)
        self.b.append(0x9B); self.align()
        hdr = len(self.b); self.b += p32(name_ptr) + p32(0) + p32(params_ptr) + p32(0)
        self.b += bytes.fromhex('9876543212345678')
        start = len(self.b); body_fn(); self.b[hdr + 12:hdr + 16] = p32(len(self.b) - start)


def patch_apt(data, const):
    data = bytearray(data); const = bytearray(const)
    # --- constant file: append strings + a new, longer items array
    n_items, items_off = u32(const, 0x18), u32(const, 0x1C)
    items = bytearray(const[items_off:items_off + n_items * 8])
    strings = {}
    names = [] if COPY_ONLY else [TAB_NAME] + [t for t, _ in ROWS] + [l for _, ls in ROWS for l in ls]
    for s in dict.fromkeys(names):
        strings[s] = len(const); const += s.encode('latin1') + b'\0'
    while len(const) % 4: const.append(0)
    new_index = {}
    for s, off in strings.items():
        new_index[s] = n_items + len(new_index); items += p32(1) + p32(off)
    const[0x1C:0x20] = p32(len(const)); const[0x18:0x1C] = p32(n_items + len(new_index))
    const += items

    # --- data file: the frame-0 action
    # movie character (offset in const file @0x14): +16 frame count, +20 frames ptr;
    # frame0 = (item count, items ptr); action item = (type 1, bytecode ptr)
    movie = u32(const, 0x14)
    assert u32(data, movie + 4) == 0x09876543
    frames = u32(data, movie + 20); icnt, iptr = u32(data, frames), u32(data, frames + 4)
    act_item = None
    for k in range(icnt):
        it = u32(data, iptr + 4 * k)
        if u32(data, it) == 1: act_item = it
    assert act_item is not None
    old = u32(data, act_item + 4)
    assert data[old] == 0x88, 'frame action does not start with ConstantPool'
    pool_n, pool_ptr = u32(data, old + 4), u32(data, old + 8)
    pool = [u32(data, pool_ptr + 4 * i) for i in range(pool_n)]
    # find the end of the top-level stream (walk with the disassembler)
    from aptdis import Const, disasm
    out = []; end = disasm(bytes(data), Const(bytes(const)), old, out) - 1
    assert data[end] == 0x00 and out[-1].endswith('End'), out[-1]

    def pool_ix(s):
        if s in new_index: return pool_n + list(new_index).index(s)
        for i, ci in enumerate(pool):
            t, v = u32(const, items_off + ci * 8), u32(const, items_off + ci * 8 + 4)
            if t == 1 and const[v:const.index(b'\0', v)].decode('latin1') == s: return i
        raise KeyError(s)

    # template DefineFunction pointers (anonymous, no params) from the original code
    fn_off = int(next(l for l in out if 'function1  size=' in l).split()[0], 16)  # anonymous one
    assert data[fn_off] == 0x9B
    q = (fn_off + 1 + 3) & ~3
    name_ptr, params_ptr = u32(data, q), u32(data, q + 8)

    while len(data) % 8: data.append(0)
    new = len(data)
    assert new % 8 == old % 8
    a = Asm(new, pool_ix)
    a.b.append(0x88); a.align(); patch_at = len(a.b); a.b += p32(0) + p32(0)
    a.b += data[old + 12:end]                     # the original statements, verbatim
    assert (old + 12) % 4 == (new + 12) % 4

    if COPY_ONLY:
        a.op(0x00); data += a.b
        while len(data) % 4: data.append(0)
        idx_ptr = len(data)
        for ci in pool: data += p32(ci)
        data[new + patch_at:new + patch_at + 8] = p32(pool_n) + p32(idx_ptr)
        data[act_item + 4:act_item + 8] = p32(new)
        return bytes(data), bytes(const), new
    # categories[200] = new Object(); .name = "DISPLAY"; .settings = [201, ...]
    a.var('categories'); a.int_(CATEGORY); a.int_(0); a.const('Object'); a.op(0x40); a.op(0x4F)
    a.var('categories'); a.int_(CATEGORY); a.op(0x4E); a.const('name'); a.const(TAB_NAME); a.op(0x4F)
    a.var('categories'); a.int_(CATEGORY); a.op(0x4E); a.const('settings')
    ids = [CATEGORY + 1 + i for i in range(len(ROWS))]
    for sid in reversed(ids): a.int_(sid)
    a.int_(len(ids)); a.op(0x42); a.op(0x4F)
    # initSettings(id, title, "toggle", labelsFn, valuesFn, true)
    for sid, (title, labels) in zip(ids, ROWS):
        a.op(0x73)                                                    # isEnabled = true
        def values_body(n=len(labels)):
            for v in reversed(range(n)): a.int_(v)
            a.int_(n); a.const('Array'); a.op(0x40); a.op(0x3E)
        def labels_body(ls=labels):
            for s in reversed(ls): a.const(s)
            a.int_(len(ls)); a.const('Array'); a.op(0x40); a.op(0x3E)
        a.function(values_body, name_ptr, params_ptr)
        a.function(labels_body, name_ptr, params_ptr)
        a.const('toggle'); a.const(title); a.int_(sid); a.int_(6); a.byte_op(0xB0, 'initSettings')
    # context[Settings.eContext.X].push(200)
    for c in CONTEXTS:
        a.int_(CATEGORY); a.int_(1)
        a.var('context'); a.op(0x71); a.member('Settings'); a.member('eContext'); a.member(c); a.op(0x4E)
        a.byte_op(0xB2, 'push')
    a.op(0x00)                                                        # End
    data += a.b
    while len(data) % 4: data.append(0)
    idx_ptr = len(data)
    for ci in pool: data += p32(ci)
    for s in new_index: data += p32(new_index[s])
    data[new + patch_at:new + patch_at + 8] = p32(pool_n + len(new_index)) + p32(idx_ptr)
    data[act_item + 4:act_item + 8] = p32(new)
    return bytes(data), bytes(const), new


def main():
    fe = sys.argv[1]
    orig = fe + '.orig'
    if not os.path.exists(orig):
        print('backing up', fe, '->', orig); shutil.copyfile(fe, orig)
    idx = read_index(orig)
    i = next(k for k, e in enumerate(idx) if e[0] == MEMBER)
    nm, path, off, size, _ = idx[i]
    with open(orig, 'rb') as f:
        f.seek(off); big = f.read(size)
    mem, footer = bigf_read(big)
    dm = {n: unrefpack(d) for n, d in mem}
    data, const, new = patch_apt(dm['0'], dm['1'])
    # verify by disassembling the new frame action
    from aptdis import Const, disasm
    out = []; disasm(data, Const(const), new, out)
    assert out[-1].endswith('End') and not any('unknown' in l for l in out)
    for m in mem:
        if m[0] == '0': m[1] = refpack(data)
        elif m[0] == '1': m[1] = refpack(const)
    newbig = bigf_write(mem, footer)
    shutil.copyfile(orig, fe + '.tmp')
    with open(fe + '.tmp', 'r+b') as f:
        f.seek(0, 2); end = f.tell(); pad = (-end) % 16
        f.write(bytes(pad)); at = end + pad; f.write(newbig); f.write(bytes((-len(newbig)) % 16))
        ent = 0x30 + i * 16
        f.seek(ent); e = f.read(16)
        f.seek(ent); f.write(p32(at >> 4) + e[4:8] + p32(len(newbig)) + e[12:16])
        f.seek(0, 2); new_size = f.tell()
        f.seek(0x1C); hdr_size = u32(f.read(4), 0)
        if hdr_size == end:  # EB header field 0x1C = archive size
            f.seek(0x1C); f.write(p32(new_size))
    os.replace(fe + '.tmp', fe)
    print(f'patched {MEMBER}: new frame action @0x{new:X}, {len(out)} ops; member @0x{at:X} size {len(newbig)}')
    print(f'check: {read_index(fe)[i]}')


if __name__ == '__main__':
    main()
