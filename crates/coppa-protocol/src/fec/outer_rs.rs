//! Systematic RS(12,6) erasure code over GF(256), SPEC §16.

#[derive(Debug)]
struct Gf256 {
    exp: [u8; 512],
    log: [u8; 256],
}
impl Gf256 {
    fn new() -> Self {
        let mut exp = [0; 512];
        let mut log = [0; 256];
        let mut x = 1u16;
        for (i, slot) in exp.iter_mut().enumerate().take(255) {
            *slot = x as u8;
            log[x as usize] = i as u8;
            x <<= 1;
            if x & 256 != 0 {
                x ^= 0x11d;
            }
        }
        for i in 255..512 {
            exp[i] = exp[i - 255];
        }
        Self { exp, log }
    }
    fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            0
        } else {
            self.exp[self.log[a as usize] as usize + self.log[b as usize] as usize]
        }
    }
    fn inv(&self, a: u8) -> Option<u8> {
        (a != 0).then(|| self.exp[255 - self.log[a as usize] as usize])
    }
}

/// Six systematic shards and six Cauchy parity shards. See SPEC §16.
#[derive(Debug)]
pub struct OuterRs12x6 {
    gf: Gf256,
    generator: [[u8; 6]; 12],
}
impl Default for OuterRs12x6 {
    fn default() -> Self {
        Self::new()
    }
}
impl OuterRs12x6 {
    /// Construct the fixed code over polynomial 0x11d.
    pub fn new() -> Self {
        let gf = Gf256::new();
        let mut generator = [[0; 6]; 12];
        for (r, row) in generator.iter_mut().enumerate() {
            if r < 6 {
                row[r] = 1;
            } else {
                for (i, c) in row.iter_mut().enumerate() {
                    *c = gf.inv(r as u8 ^ i as u8).unwrap_or(0);
                }
            }
        }
        Self { gf, generator }
    }
    /// Encode equal-length data shards. Unequal inputs produce empty shards.
    pub fn encode(&self, data: &[&[u8]; 6]) -> [Vec<u8>; 12] {
        let len = data[0].len();
        if data.iter().any(|s| s.len() != len) {
            return std::array::from_fn(|_| Vec::new());
        }
        std::array::from_fn(|r| {
            (0..len)
                .map(|b| {
                    (0..6).fold(0, |acc, i| {
                        acc ^ self.gf.mul(self.generator[r][i], data[i][b])
                    })
                })
                .collect()
        })
    }
    /// Rebuild from at least six distinct, valid, equal-length shards.
    pub fn decode(&self, available: &[(usize, &[u8])]) -> Option<[Vec<u8>; 6]> {
        if available.len() < 6 || available.len() > 12 {
            return None;
        }
        let len = available[0].1.len();
        let mut seen = [false; 12];
        for &(i, s) in available {
            if i >= 12 || seen[i] || s.len() != len {
                return None;
            }
            seen[i] = true;
        }
        let used = &available[..6];
        let mut m = std::array::from_fn::<_, 6, _>(|i| self.generator[used[i].0]);
        let mut inv = [[0; 6]; 6];
        for (i, row) in inv.iter_mut().enumerate() {
            row[i] = 1;
        }
        for col in 0..6 {
            let pivot = (col..6).find(|&r| m[r][col] != 0)?;
            m.swap(col, pivot);
            inv.swap(col, pivot);
            let scale = self.gf.inv(m[col][col])?;
            for c in 0..6 {
                m[col][c] = self.gf.mul(m[col][c], scale);
                inv[col][c] = self.gf.mul(inv[col][c], scale);
            }
            for r in 0..6 {
                if r != col {
                    let factor = m[r][col];
                    for c in 0..6 {
                        m[r][c] ^= self.gf.mul(factor, m[col][c]);
                        inv[r][c] ^= self.gf.mul(factor, inv[col][c]);
                    }
                }
            }
        }
        Some(std::array::from_fn(|i| {
            (0..len)
                .map(|b| (0..6).fold(0, |acc, k| acc ^ self.gf.mul(inv[i][k], used[k].1[b])))
                .collect()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_924_subsets_and_all_five_packet_sets() {
        let code = OuterRs12x6::new();
        for len in [1, 99] {
            let data: [Vec<u8>; 6] = std::array::from_fn(|i| {
                (0..len)
                    .map(|b| ((i * 73 + b * 19 + 7) % 256) as u8)
                    .collect()
            });
            let shards = code.encode(&std::array::from_fn(|i| data[i].as_slice()));
            let mut count = 0;
            for mask in 0u16..4096 {
                let n = mask.count_ones();
                if n != 5 && n != 6 {
                    continue;
                }
                let available: Vec<_> = (0..12)
                    .filter(|i| mask & (1 << i) != 0)
                    .map(|i| (i, shards[i].as_slice()))
                    .collect();
                if n == 6 {
                    assert_eq!(code.decode(&available), Some(data.clone()));
                    count += 1;
                } else {
                    assert!(code.decode(&available).is_none());
                }
            }
            assert_eq!(count, 924);
        }
    }
    #[test]
    fn known_answer_vector() {
        let encoded = OuterRs12x6::new().encode(&[&[1], &[2], &[3], &[4], &[5], &[6]]);
        assert_eq!(
            encoded.map(|s| s[0]),
            [1, 2, 3, 4, 5, 6, 242, 187, 184, 138, 51, 44]
        );
    }
    #[test]
    fn invalid_inputs_are_refused() {
        let code = OuterRs12x6::new();
        assert!(code.decode(&[(0, &[1][..]); 6]).is_none());
        assert!(code
            .decode(&[
                (0, &[1]),
                (1, &[1]),
                (2, &[1]),
                (3, &[1]),
                (4, &[1]),
                (12, &[1])
            ])
            .is_none());
        assert!(code
            .decode(&[
                (0, &[1]),
                (1, &[1]),
                (2, &[1]),
                (3, &[1]),
                (4, &[1]),
                (5, &[])
            ])
            .is_none());
    }
}
