//! hub: the sole external gate (arch sec 6).
//!
//! Law #3: every external relationship of the Robot -- model APIs, search,
//! web fetch, Telegram -- is held here and nowhere else; there is no code
//! path to a socket that bypasses the Gateway, and therefore none that
//! bypasses the Boundary Log.
//!
//! M1 ships the chokepoint type only, and that is meaningful in itself: a
//! Robot with zero connectors configured is fully self-contained on its
//! machine (arch sec 6) -- it talks to no one and is reachable only through
//! the built-in Chat. M4 fills in OpenRouter, Serper, and the fetch/READ loop.

#[derive(Default)]
pub struct Gateway {
    _sealed: (),
}

impl Gateway {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of external endpoints configured. M1: zero, by design.
    pub fn endpoints(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m1_gateway_is_self_contained() {
        assert_eq!(Gateway::new().endpoints(), 0);
    }
}
