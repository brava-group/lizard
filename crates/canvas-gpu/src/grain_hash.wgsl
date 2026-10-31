//! 64-bit grain hash, matching lizard-render::grain on the GPU.
struct Word64 { lo: u32, hi: u32 };

fn product(a: Word64, b: Word64) -> Word64 {
    let a0 = a.lo & 65535u;
    let a1 = a.lo >> 16u;
    let b0 = b.lo & 65535u;
    let b1 = b.lo >> 16u;
    let p0 = a0 * b0;
    let p1 = a0 * b1;
    let p2 = a1 * b0;
    let p3 = a1 * b1;
    let mid = (p0 >> 16u) + (p1 & 65535u) + (p2 & 65535u);
    return Word64((p0 & 65535u) | (mid << 16u), p3 + (p1 >> 16u) + (p2 >> 16u) + (mid >> 16u) + a.lo * b.hi + a.hi * b.lo);
}

fn grain_hash(ix: i32, iy: i32, seed: u32) -> Word64 {
    let x = Word64(bitcast<u32>(ix), select(0u, 0xffffffffu, ix < 0));
    let y = Word64(bitcast<u32>(iy), select(0u, 0xffffffffu, iy < 0));
    let sx = product(x, Word64(0x7f4a7c15u, 0x9e3779b9u));
    let sy = product(y, Word64(0x27d4eb4fu, 0xc2b2ae3du));
    let ss = product(Word64(seed, 0u), Word64(0x9e3779f9u, 0x165667b1u));
    var h = Word64(sx.lo ^ sy.lo ^ ss.lo, sx.hi ^ sy.hi ^ ss.hi);
    h.lo = h.lo ^ (h.hi >> 1u);
    h = product(h, Word64(0xed558ccdu, 0xff51afd7u));
    h.lo = h.lo ^ (h.hi >> 1u);
    h = product(h, Word64(0x1a85ec53u, 0xc4ceb9feu));
    h.lo = h.lo ^ (h.hi >> 1u);
    return h;
}

fn unit(h: Word64, at: u32) -> f32 {
    if at == 0u { return f32(h.lo & 65535u) / 65536.0; }
    if at == 16u { return f32(h.lo >> 16u) / 65536.0; }
    if at == 32u { return f32(h.hi & 65535u) / 65536.0; }
    return f32(h.hi >> 16u) / 65536.0;
}
