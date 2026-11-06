//! Private FX.25 wire and Reed-Solomon primitives. Polynomials use GF(256),
//! primitive polynomial 0x11d, with consecutive generator roots alpha^1 onward.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Profile {
    pub tag: u64,
    pub data: usize,
    pub parity: usize,
}
pub(crate) const PROFILES: [Profile; 11] = [
    Profile {
        tag: 0xB74DB7DF8A532F3E,
        data: 239,
        parity: 16,
    },
    Profile {
        tag: 0x26FF60A600CC8FDE,
        data: 128,
        parity: 16,
    },
    Profile {
        tag: 0xC7DC0508F3D9B09E,
        data: 64,
        parity: 16,
    },
    Profile {
        tag: 0x8F056EB4369660EE,
        data: 32,
        parity: 16,
    },
    Profile {
        tag: 0x6E260B1AC5835FAE,
        data: 223,
        parity: 32,
    },
    Profile {
        tag: 0xFF94DC634F1CFF4E,
        data: 128,
        parity: 32,
    },
    Profile {
        tag: 0x1EB7B9CDBC09C00E,
        data: 64,
        parity: 32,
    },
    Profile {
        tag: 0xDBF869BD2DBB1776,
        data: 32,
        parity: 32,
    },
    Profile {
        tag: 0x3ADB0C13DEAE2836,
        data: 191,
        parity: 64,
    },
    Profile {
        tag: 0xAB69DB6A543188D6,
        data: 128,
        parity: 64,
    },
    Profile {
        tag: 0x4A4ABEC4A724B796,
        data: 64,
        parity: 64,
    },
];
pub(crate) fn find_tag(tag: u64) -> Option<Profile> {
    PROFILES
        .iter()
        .copied()
        .find(|p| (p.tag ^ tag).count_ones() <= 8)
}
fn mul(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        a = (a << 1) ^ if a & 0x80 != 0 { 0x1d } else { 0 };
        b >>= 1;
    }
    r
}
fn pow(mut a: u8, mut n: usize) -> u8 {
    let mut r = 1;
    while n != 0 {
        if n & 1 != 0 {
            r = mul(r, a);
        }
        a = mul(a, a);
        n >>= 1;
    }
    r
}
fn eval(poly: &[u8], x: u8) -> u8 {
    poly.iter().fold(0, |v, &c| mul(v, x) ^ c)
}
/// Systematic polynomial division, high coefficient first. Shortening removes
/// trailing data zeros, so those zeros still participate in parity generation.
pub(crate) fn encode(data: &[u8], p: Profile) -> Vec<u8> {
    assert_eq!(data.len(), p.data);
    let mut generator = vec![1];
    for i in 1..=p.parity {
        let mut next = vec![0; generator.len() + 1];
        for (j, &v) in generator.iter().enumerate() {
            next[j] ^= v;
            next[j + 1] ^= mul(v, pow(2, i));
        }
        generator = next;
    }
    let mut word = [0; 255];
    word[..data.len()].copy_from_slice(data);
    for i in 0..255 - p.parity {
        let c = word[i];
        for j in 1..=p.parity {
            word[i + j] ^= mul(c, generator[j]);
        }
    }
    let mut out = data.to_vec();
    out.extend_from_slice(&word[255 - p.parity..]);
    out
}
pub(crate) fn decode(block: &[u8], p: Profile) -> Option<Vec<u8>> {
    if block.len() != p.data + p.parity {
        return None;
    }
    let mut word = [0; 255];
    word[..p.data].copy_from_slice(&block[..p.data]);
    word[255 - p.parity..].copy_from_slice(&block[p.data..]);
    let synd: Vec<_> = (1..=p.parity).map(|i| eval(&word, pow(2, i))).collect();
    if synd.iter().all(|&v| v == 0) {
        return Some(word[..p.data].to_vec());
    }
    // Berlekamp-Massey over the syndrome sequence; locator is low order first.
    let mut c = vec![0; p.parity + 1];
    c[0] = 1;
    let mut b = c.clone();
    let (mut degree, mut shift, mut previous) = (0, 1, 1);
    for n in 0..p.parity {
        let mut d = synd[n];
        for i in 1..=degree {
            d ^= mul(c[i], synd[n - i]);
        }
        if d == 0 {
            shift += 1;
            continue;
        }
        let old = c.clone();
        let scale = mul(d, pow(previous, 254));
        for i in 0..=p.parity - shift {
            c[i + shift] ^= mul(scale, b[i]);
        }
        if 2 * degree <= n {
            degree = n + 1 - degree;
            b = old;
            previous = d;
            shift = 1;
        } else {
            shift += 1;
        }
    }
    if degree == 0 || degree > p.parity / 2 {
        return None;
    }
    let mut positions = Vec::new();
    for pos in 0..255 {
        let inv = pow(2, (255 - (254 - pos)) % 255);
        let value = c[..=degree].iter().rev().fold(0, |v, &a| mul(v, inv) ^ a);
        if value == 0 {
            positions.push(pos);
        }
    }
    if positions.len() != degree {
        return None;
    }
    // Solve the first 'degree' syndrome equations for error magnitudes.
    let mut matrix = vec![vec![0; degree + 1]; degree];
    for (row, values) in matrix.iter_mut().enumerate() {
        for (col, &pos) in positions.iter().enumerate() {
            values[col] = pow(2, ((254 - pos) * (row + 1)) % 255);
        }
        values[degree] = synd[row];
    }
    for col in 0..degree {
        let pivot = (col..degree).find(|&r| matrix[r][col] != 0)?;
        matrix.swap(col, pivot);
        let inv = pow(matrix[col][col], 254);
        for v in &mut matrix[col][col..] {
            *v = mul(*v, inv);
        }
        for r in 0..degree {
            if r == col {
                continue;
            }
            let scale = matrix[r][col];
            let pivot = matrix[col].clone();
            for (value, &coefficient) in matrix[r][col..].iter_mut().zip(&pivot[col..]) {
                *value ^= mul(scale, coefficient);
            }
        }
    }
    for (i, &pos) in positions.iter().enumerate() {
        word[pos] ^= matrix[i][degree];
    }
    if (1..=p.parity).any(|i| eval(&word, pow(2, i)) != 0)
        || word[p.data..255 - p.parity].iter().any(|&v| v != 0)
    {
        return None;
    }
    Some(word[..p.data].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_vectors_and_correction_bounds() {
        for (i, p) in PROFILES.iter().copied().enumerate() {
            let path = format!(
                "{}/tests/fixtures/fx25/tag{:02}.bin",
                env!("CARGO_MANIFEST_DIR"),
                i + 1
            );
            let block = std::fs::read(path).unwrap();
            assert_eq!(encode(&block[..p.data], p), block);
            for count in [0, 1, p.parity / 2] {
                let mut damaged = block.clone();
                for n in 0..count {
                    let pos = if n % 2 == 0 {
                        n / 2
                    } else {
                        block.len() - 1 - n / 2
                    };
                    damaged[pos] ^= 0xff;
                }
                assert_eq!(decode(&damaged, p).unwrap(), block[..p.data]);
            }
            let mut damaged = block.clone();
            for (j, v) in damaged.iter_mut().take(p.parity).enumerate() {
                *v ^= (j + 1) as u8;
            }
            assert!(decode(&damaged, p).is_none());
        }
    }
    #[test]
    fn tag_distance_and_reserved_tags() {
        assert!(find_tag(PROFILES[0].tag).is_some());
        assert!(find_tag(PROFILES[0].tag ^ 0xff).is_some());
        assert!(find_tag(PROFILES[0].tag ^ 0x1ff).is_none());
        for t in [
            0x566ED2717946107E,
            0x0293D578626B67E6,
            0xE3B0B0D6917E58A6,
            0x720267AF1BE1F846,
            0x93210201E8F4C706,
        ] {
            assert!(find_tag(t).is_none());
        }
    }
}
