// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SHA-256, in-house.
//!
//! Spec: ../../specs/015-ota.md
//!
//! Why not a crate: the only client is the check of the downloaded
//! package, and that check has to run **the same** on all three systems --
//! including Windows, where the system equivalent would be
//! `certutil -hashfile` with its own output to parse. Three tools and
//! three parsers to compare 64 hex characters would be more code than the
//! algorithm, and more places to get it wrong.
//!
//! It is not a new dependency across three toolchains (one of them
//! cross-compiled for mingw, another on the Mac) for 80 lines that
//! FIPS 180-4 fixed in 2002 and that the vectors below pin down. The
//! expected values in the tests came from this machine's `sha256sum`,
//! not from my memory.

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const INITIAL: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

fn compress(h: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([block[i * 4], block[i * 4 + 1], block[i * 4 + 2], block[i * 4 + 3]]);
    }
    for i in 16..64 {
        let a = w[i - 15];
        let b = w[i - 2];
        let s0 = a.rotate_right(7) ^ a.rotate_right(18) ^ (a >> 3);
        let s1 = b.rotate_right(17) ^ b.rotate_right(19) ^ (b >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut t] = *h;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choose = (e & f) ^ ((!e) & g);
        let t1 = t
            .wrapping_add(s1)
            .wrapping_add(choose)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(majority);
        t = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (i, v) in [a, b, c, d, e, f, g, t].iter().enumerate() {
        h[i] = h[i].wrapping_add(*v);
    }
}

/// The raw digest of the bytes.
///
/// Walks the input in 64-byte blocks without copying anything: a package
/// is tens of MB, and duplicating that in a laptop's memory to compute a
/// hash would be pointless waste.
pub fn digest(data: &[u8]) -> [u8; 32] {
    let mut h = INITIAL;
    let mut blocks = data.chunks_exact(64);
    for b in &mut blocks {
        compress(&mut h, b);
    }

    // The remainder, 0x80, zeros, and the length in bits at the end. It
    // fits in one block when fewer than 56 bytes are left; in two if not.
    let rest = blocks.remainder();
    let mut tail = [0u8; 128];
    tail[..rest.len()].copy_from_slice(rest);
    tail[rest.len()] = 0x80;
    let n = if rest.len() < 56 { 64 } else { 128 };
    let bits = (data.len() as u64).wrapping_mul(8);
    tail[n - 8..n].copy_from_slice(&bits.to_be_bytes());
    for b in tail[..n].chunks_exact(64) {
        compress(&mut h, b);
    }

    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// The digest in lowercase hex, in the format `SHA256SUMS` carries.
pub fn hex(data: &[u8]) -> String {
    let mut s = String::with_capacity(64);
    for b in digest(data) {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aa(n: usize) -> Vec<u8> {
        vec![b'a'; n]
    }

    // The sizes are the ones that touch padding: 55 still fits in the
    // block, 56 forces a second one, 64 is a full block, 119/120 repeat
    // the boundary one block further. Expected values measured with
    // `sha256sum`.
    #[test]
    fn padding_boundaries() {
        let cases: [(usize, &str); 11] = [
            (0, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            (1, "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb"),
            (55, "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318"),
            (56, "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a"),
            (57, "f13b2d724659eb3bf47f2dd6af1accc87b81f09f59f2b75e5c0bed6589dfe8c6"),
            (63, "7d3e74a05d7db15bce4ad9ec0658ea98e3f06eeecf16b4c6fff2da457ddc2f34"),
            (64, "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"),
            (65, "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0"),
            (119, "31eba51c313a5c08226adf18d4a359cfdfd8d2e816b13f4af952f7ea6584dcfb"),
            (120, "2f3d335432c70b580af0e8e1b3674a7c020d683aa5f73aaaedfdc55af904c21c"),
            (121, "e9615320128cc7a3d6078e9af05603188e5ccbf0d07d8b735d3df5e8e0c1281f"),
        ];
        for (n, expected) in cases {
            assert_eq!(hex(&aa(n)), expected, "{n} bytes of 'a'");
        }
    }

    #[test]
    fn the_usual_vectors() {
        assert_eq!(hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    /// A real package has many blocks; 1000 bytes is already over 15.
    #[test]
    fn many_blocks() {
        assert_eq!(hex(&aa(1000)), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
    }

    /// Real package size (3 MB), to exercise the block loop end to end
    /// and not just the padding. Expected value computed with Python's
    /// `hashlib` over the same bytes.
    #[test]
    fn package_sized_input() {
        let d: Vec<u8> = (0u64..3_000_001).map(|i| ((i * i * 31 + i * 7) % 256) as u8).collect();
        assert_eq!(hex(&d), "56484b4772f04b6fe7a80b8fafe150e778dddec70751d70173d82fae002df7e6");
    }

    /// One flipped bit has to give another digest -- the only thing the
    /// package check actually asks for.
    #[test]
    fn one_bit_changes_everything() {
        let a = hex(b"oxdesk-linux-x86_64-124-abc1234.tar.gz");
        let b = hex(b"oxdesk-linux-x86_64-124-abc1235.tar.gz");
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
    }
}
