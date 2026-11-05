//! ISAAC stream cipher, as used by the 274 game protocol.
//!
//! Ports the server implementation field by field. All arithmetic follows
//! the original 32-bit wrapping rules so both sides stay in sync.

const MASK: u32 = 0xFFFF_FFFF;

fn signed(word: u32) -> i32 {
    (word & MASK) as i32
}

fn unsigned(word: i32) -> u32 {
    word as u32
}

/// ISAAC keystream: one [`next_int`](Isaac::next_int) per opcode byte.
#[derive(Debug, Clone)]
pub struct Isaac {
    count: usize,
    results: [i32; 256],
    memory: [i32; 256],
    reg_a: i32,
    reg_b: i32,
    reg_c: i32,
}

impl Isaac {
    pub fn new(seed: [i32; 4]) -> Isaac {
        let mut cipher = Isaac {
            count: 0,
            results: [0; 256],
            memory: [0; 256],
            reg_a: 0,
            reg_b: 0,
            reg_c: 0,
        };
        for (index, word) in seed.iter().enumerate().take(256) {
            cipher.results[index] = *word;
        }
        cipher.init();
        cipher
    }

    pub fn next_int(&mut self) -> i32 {
        if self.count == 0 {
            self.cycle();
            self.count = 255;
        } else {
            self.count -= 1;
        }
        self.results[self.count]
    }

    #[allow(clippy::too_many_arguments)]
    fn init(&mut self) {
        let mut regs = [0x9E37_79B9u32; 8];
        for _ in 0..4 {
            regs = mix(regs);
        }
        for chunk in 0..32 {
            let base = chunk * 8;
            for lane in 0..8 {
                regs[lane] = regs[lane].wrapping_add(unsigned(self.results[base + lane]));
            }
            regs = mix(regs);
            self.memory[base] = signed(regs[0]);
            self.memory[base + 1] = signed(regs[1]);
            self.memory[base + 2] = signed(regs[2]);
            self.memory[base + 3] = signed(regs[3]);
            self.memory[base + 4] = signed(regs[4]);
            self.memory[base + 5] = signed(regs[5]);
            self.memory[base + 6] = signed(regs[6]);
            self.memory[base + 7] = signed(regs[7]);
        }
        for chunk in 0..32 {
            let base = chunk * 8;
            for lane in 0..8 {
                regs[lane] = regs[lane].wrapping_add(unsigned(self.memory[base + lane]));
            }
            regs = mix(regs);
            self.memory[base] = signed(regs[0]);
            self.memory[base + 1] = signed(regs[1]);
            self.memory[base + 2] = signed(regs[2]);
            self.memory[base + 3] = signed(regs[3]);
            self.memory[base + 4] = signed(regs[4]);
            self.memory[base + 5] = signed(regs[5]);
            self.memory[base + 6] = signed(regs[6]);
            self.memory[base + 7] = signed(regs[7]);
        }
        self.cycle();
        self.count = 256;
    }

    fn cycle(&mut self) {
        self.reg_c = self.reg_c.wrapping_add(1);
        self.reg_b = self.reg_b.wrapping_add(self.reg_c);
        for index in 0..256 {
            let x = self.memory[index];
            match index & 3 {
                0 => self.reg_a ^= signed(unsigned(self.reg_a).wrapping_shl(13)),
                1 => self.reg_a ^= signed(unsigned(self.reg_a).wrapping_shr(6)),
                2 => self.reg_a ^= signed(unsigned(self.reg_a).wrapping_shl(2)),
                _ => self.reg_a ^= signed(unsigned(self.reg_a).wrapping_shr(16)),
            }
            self.reg_a = signed(unsigned(self.reg_a).wrapping_add(unsigned(self.memory[(index + 128) & 0xFF])));
            let slot = (unsigned(x).wrapping_shr(2) & 0xFF) as usize;
            let y = signed(unsigned(self.memory[slot]).wrapping_add(unsigned(self.reg_a)).wrapping_add(unsigned(self.reg_b)));
            self.memory[index] = y;
            let slot = (unsigned(y).wrapping_shr(10) & 0xFF) as usize;
            self.reg_b = signed(unsigned(self.memory[slot]).wrapping_add(unsigned(x)));
            self.results[index] = self.reg_b;
        }
    }
}

fn mix(regs: [u32; 8]) -> [u32; 8] {
    // All server arithmetic is mod 2^32 (signed/unsigned views only
    // reinterpret the same bits), so plain wrapping u32 ops are exact.
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = regs;
    a ^= b.wrapping_shl(11);
    d = d.wrapping_add(a);
    b = b.wrapping_add(c);
    b ^= c.wrapping_shr(2);
    e = e.wrapping_add(b);
    c = c.wrapping_add(d);
    c ^= d.wrapping_shl(8);
    f = f.wrapping_add(c);
    d = d.wrapping_add(e);
    d ^= e.wrapping_shr(16);
    g = g.wrapping_add(d);
    e = e.wrapping_add(f);
    e ^= f.wrapping_shl(10);
    h = h.wrapping_add(e);
    f = f.wrapping_add(g);
    f ^= g.wrapping_shr(4);
    a = a.wrapping_add(f);
    g = g.wrapping_add(h);
    g ^= h.wrapping_shl(8);
    b = b.wrapping_add(g);
    h = h.wrapping_add(a);
    h ^= a.wrapping_shr(9);
    c = c.wrapping_add(h);
    a = a.wrapping_add(b);
    [a, b, c, d, e, f, g, h]
}

#[cfg(test)]
mod tests {
    use super::Isaac;

    #[test]
    fn same_seed_same_stream() {
        let mut first = Isaac::new([1, 2, 3, 4]);
        let mut second = Isaac::new([1, 2, 3, 4]);
        for _ in 0..600 {
            assert_eq!(first.next_int(), second.next_int());
        }
    }

    #[test]
    fn zero_seed_matches_reference() {
        // Captured from lifescape.isaac.Isaac((0, 0, 0, 0)).
        let expected = [
            405143795,
            806046349,
            807101986,
            -1333080799,
            695195257,
            -1722677527,
            -1275090763,
            264870948,
        ];
        let mut cipher = Isaac::new([0, 0, 0, 0]);
        for want in expected {
            assert_eq!(cipher.next_int(), want);
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut first = Isaac::new([1, 2, 3, 4]);
        let mut second = Isaac::new([5, 6, 7, 8]);
        let head: Vec<i32> = (0..16).map(|_| first.next_int()).collect();
        let other: Vec<i32> = (0..16).map(|_| second.next_int()).collect();
        assert_ne!(head, other);
    }
}
