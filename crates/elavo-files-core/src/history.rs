//! Back and Forward, per window.

/// Places visited, with the current one in `current`.
#[derive(Clone, Debug)]
pub struct History<T> {
    back: Vec<T>,
    current: T,
    forward: Vec<T>,
}

impl<T: Clone + PartialEq> History<T> {
    pub fn new(start: T) -> Self {
        Self {
            back: Vec::new(),
            current: start,
            forward: Vec::new(),
        }
    }

    pub fn current(&self) -> &T {
        &self.current
    }

    /// Go somewhere new. Forward is forgotten. Going to where you already
    /// are does nothing and returns false.
    pub fn visit(&mut self, place: T) -> bool {
        if place == self.current {
            return false;
        }
        let old = std::mem::replace(&mut self.current, place);
        self.back.push(old);
        self.forward.clear();
        true
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn back(&mut self) -> Option<&T> {
        let previous = self.back.pop()?;
        let old = std::mem::replace(&mut self.current, previous);
        self.forward.push(old);
        Some(&self.current)
    }

    pub fn forward(&mut self) -> Option<&T> {
        let next = self.forward.pop()?;
        let old = std::mem::replace(&mut self.current, next);
        self.back.push(old);
        Some(&self.current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_and_forward() {
        let mut h = History::new("home");
        assert!(!h.can_go_back());
        assert!(h.visit("Desktop"));
        assert!(h.visit("Desktop/Projects"));
        assert!(!h.visit("Desktop/Projects"));
        assert_eq!(h.back(), Some(&"Desktop"));
        assert_eq!(h.back(), Some(&"home"));
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), Some(&"Desktop"));
        assert!(h.can_go_forward());
        h.visit("Downloads");
        assert!(!h.can_go_forward());
        assert_eq!(h.current(), &"Downloads");
        assert_eq!(h.back(), Some(&"Desktop"));
    }
}
