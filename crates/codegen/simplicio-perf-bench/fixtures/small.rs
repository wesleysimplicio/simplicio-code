//! Golden fixture: small.rs
//! Deliberately tiny so latency floors dominate the measurement.

pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

pub fn sub(a: i64, b: i64) -> i64 {
    a - b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds() {
        assert_eq!(add(2, 3), 5);
    }

    #[test]
    fn subs() {
        assert_eq!(sub(5, 3), 2);
    }
}
