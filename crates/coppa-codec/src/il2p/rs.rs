//! Shortened systematic Reed-Solomon over GF(256), roots alpha^0 onward.
//! Original polynomial implementation. Decoder uses Berlekamp-Massey for
//! locations and a Vandermonde solve for magnitudes, followed by verification.
fn mul(mut a: u8, mut b: u8) -> u8 {
    let mut out = 0;
    for _ in 0..8 {
        if b & 1 != 0 {
            out ^= a;
        }
        let high = a & 0x80 != 0;
        a <<= 1;
        if high {
            a ^= 0x1d;
        }
        b >>= 1;
    }
    out
}
fn pow(mut a: u8, mut n: usize) -> u8 {
    let mut out = 1;
    while n > 0 {
        if n & 1 != 0 {
            out = mul(out, a);
        }
        a = mul(a, a);
        n >>= 1;
    }
    out
}
fn div(a: u8, b: u8) -> u8 {
    debug_assert_ne!(b, 0);
    mul(a, pow(b, 254))
}
fn eval(p: &[u8], x: u8) -> u8 {
    p.iter().fold(0, |v, &c| mul(v, x) ^ c)
}
pub(super) fn encode(data: &[u8], parity: usize) -> Vec<u8> {
    let mut generator = vec![1];
    for j in 0..parity {
        let mut next = vec![0; generator.len() + 1];
        for (i, &c) in generator.iter().enumerate() {
            next[i] ^= c;
            next[i + 1] ^= mul(c, pow(2, j));
        }
        generator = next;
    }
    let mut work = data.to_vec();
    work.resize(data.len() + parity, 0);
    for i in 0..data.len() {
        let c = work[i];
        for j in 1..=parity {
            work[i + j] ^= mul(c, generator[j]);
        }
    }
    let mut result = data.to_vec();
    result.extend_from_slice(&work[data.len()..]);
    result
}
pub(super) fn correct(code: &mut [u8], parity: usize) -> Option<()> {
    if code.len() > 255 || code.len() < parity {
        return None;
    }
    let synd: Vec<u8> = (0..parity).map(|j| eval(code, pow(2, j))).collect();
    if synd.iter().all(|&s| s == 0) {
        return Some(());
    }
    // Locator coefficients are in ascending order.
    let mut c = vec![0; parity + 1];
    c[0] = 1;
    let mut b = c.clone();
    let (mut degree, mut shift, mut last) = (0, 1, 1);
    for n in 0..parity {
        let mut discrepancy = synd[n];
        for i in 1..=degree {
            discrepancy ^= mul(c[i], synd[n - i]);
        }
        if discrepancy == 0 {
            shift += 1;
            continue;
        }
        let old = c.clone();
        let scale = div(discrepancy, last);
        for i in 0..=parity - shift {
            c[i + shift] ^= mul(scale, b[i]);
        }
        if 2 * degree <= n {
            degree = n + 1 - degree;
            b = old;
            last = discrepancy;
            shift = 1;
        } else {
            shift += 1;
        }
    }
    if degree == 0 || degree * 2 > parity {
        return None;
    }
    // Search the entire mother code, so roots in omitted padding are rejected.
    let mut locations = Vec::new();
    for power in 0..255 {
        let x = pow(2, (255 - power) % 255);
        let value = c[..=degree].iter().rev().fold(0, |v, &a| mul(v, x) ^ a);
        if value == 0 {
            if power >= code.len() {
                return None;
            }
            locations.push((code.len() - 1 - power, pow(2, power)));
        }
    }
    if locations.len() != degree {
        return None;
    }
    let mut matrix = vec![vec![0; degree + 1]; degree];
    for row in 0..degree {
        for (col, &(_, x)) in locations.iter().enumerate() {
            matrix[row][col] = pow(x, row);
        }
        matrix[row][degree] = synd[row];
    }
    for col in 0..degree {
        let pivot = (col..degree).find(|&row| matrix[row][col] != 0)?;
        matrix.swap(col, pivot);
        let inverse = div(1, matrix[col][col]);
        for value in &mut matrix[col][col..=degree] {
            *value = mul(*value, inverse);
        }
        let pivot_row = matrix[col].clone();
        for (row_index, row) in matrix.iter_mut().enumerate() {
            if row_index == col {
                continue;
            }
            let factor = row[col];
            for (value, &pivot_value) in row[col..=degree].iter_mut().zip(&pivot_row[col..=degree])
            {
                *value ^= mul(factor, pivot_value);
            }
        }
    }
    for (i, &(position, _)) in locations.iter().enumerate() {
        code[position] ^= matrix[i][degree];
    }
    (0..parity)
        .all(|j| eval(code, pow(2, j)) == 0)
        .then_some(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortened_correction_at_bound() {
        for p in [2, 4, 6, 8, 16] {
            for n in [1, 13, 61, 123, 185, 239] {
                let data: Vec<u8> = (0..n).map(|i| (i * 71) as u8).collect();
                let original = encode(&data, p);
                let mut code = original.clone();
                for (i, value) in code.iter_mut().take(p / 2).enumerate() {
                    *value ^= (i + 1) as u8;
                }
                assert_eq!(correct(&mut code, p), Some(()));
                assert_eq!(code, original);
            }
        }
    }
    #[test]
    fn reject_padding_root_and_known_uncorrectable() {
        let mut full = vec![0; 255];
        full[0] = 17;
        let mut shortened = full[240..].to_vec();
        // A shortened word with the parity of an omitted nonzero symbol.
        let parity = encode(&full[..253], 2);
        shortened[13..].copy_from_slice(&parity[253..]);
        assert_eq!(correct(&mut shortened, 2), None);
        let mut code = encode(&[1; 13], 2);
        code[0] ^= 1;
        code[1] ^= 2;
        assert_eq!(correct(&mut code, 2), None);
    }
}
