//! Time types. All player-visible time is microseconds in an `i64`.

/// A point or span in time, in microseconds.
pub type Timestamp = i64;

/// A rational number, e.g. a container time base (`1/90000`) or a frame rate (`30000/1001`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational {
    /// Numerator.
    pub num: u32,
    /// Denominator (non-zero).
    pub den: u32,
}

impl Rational {
    /// Create a rational. A zero denominator is replaced by 1 so arithmetic never divides by zero.
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den: if den == 0 { 1 } else { den } }
    }

    /// Convert `ticks` of this time base (seconds per tick = `num/den`) to microseconds, rounding to nearest.
    pub fn ticks_to_us(self, ticks: i64) -> Timestamp {
        let n = ticks as i128 * self.num as i128 * 1_000_000;
        let d = self.den as i128;
        round_div(n, d) as i64
    }

    /// Convert microseconds to ticks of this time base, rounding to nearest.
    pub fn us_to_ticks(self, us: Timestamp) -> i64 {
        let n = us as i128 * self.den as i128;
        let d = self.num.max(1) as i128 * 1_000_000;
        round_div(n, d) as i64
    }

    /// Convert microseconds to ticks of this time base, rounding toward negative infinity.
    pub fn us_to_ticks_floor(self, us: Timestamp) -> i64 {
        let n = us as i128 * self.den as i128;
        let d = self.num.max(1) as i128 * 1_000_000;
        n.div_euclid(d) as i64
    }

    /// Value as `f64`.
    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }
}

fn round_div(n: i128, d: i128) -> i128 {
    let half = d / 2;
    if n >= 0 { (n + half) / d } else { (n - half) / d }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_round_trip() {
        let tb = Rational::new(1, 90_000);
        assert_eq!(tb.ticks_to_us(90_000), 1_000_000);
        assert_eq!(tb.us_to_ticks(1_000_000), 90_000);
        assert_eq!(tb.ticks_to_us(-45_000), -500_000);
        assert_eq!((tb.us_to_ticks(11_111), tb.us_to_ticks_floor(11_111)), (1000, 999));
        assert_eq!(tb.us_to_ticks_floor(-1), -1);
        let ntsc = Rational::new(1001, 30_000);
        assert_eq!(ntsc.ticks_to_us(30), 1_001_000);
    }

    #[test]
    fn zero_den_is_safe() {
        assert_eq!(Rational::new(1, 0).den, 1);
    }
}
