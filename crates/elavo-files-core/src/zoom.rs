//! Icon sizes for Ctrl+plus, Ctrl+minus and Ctrl+0, as Finder's Cmd+plus
//! and Cmd+minus.

/// Icons view sizes, in pixels.
pub const ICON_SIZES: [i32; 4] = [48, 64, 96, 128];
pub const DEFAULT_ICON: i32 = 64;

/// List view row icon sizes, in pixels.
pub const LIST_SIZES: [i32; 3] = [16, 24, 32];
pub const DEFAULT_LIST: i32 = 16;

fn index(steps: &[i32], size: i32) -> usize {
    // The nearest step, so an edited config value still works.
    steps
        .iter()
        .enumerate()
        .min_by_key(|(_, s)| (**s - size).abs())
        .map_or(0, |(i, _)| i)
}

/// The next step up, or the largest.
pub fn larger(steps: &[i32], size: i32) -> i32 {
    let i = index(steps, size);
    steps[(i + 1).min(steps.len() - 1)]
}

/// The next step down, or the smallest.
pub fn smaller(steps: &[i32], size: i32) -> i32 {
    let i = index(steps, size);
    steps[i.saturating_sub(1)]
}

/// A size read from the config, snapped to a step.
pub fn snap(steps: &[i32], size: i32) -> i32 {
    steps[index(steps, size)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_up_and_down_and_stop_at_the_ends() {
        assert_eq!(larger(&ICON_SIZES, 64), 96);
        assert_eq!(larger(&ICON_SIZES, 128), 128);
        assert_eq!(smaller(&ICON_SIZES, 64), 48);
        assert_eq!(smaller(&ICON_SIZES, 48), 48);
        assert_eq!(snap(&ICON_SIZES, 70), 64);
        assert_eq!(snap(&ICON_SIZES, 1000), 128);
        assert_eq!(larger(&LIST_SIZES, 16), 24);
        assert_eq!(smaller(&LIST_SIZES, 16), 16);
    }
}
